use crate::utils::span_lint;
use rustc::declare_lint_pass;
use rustc::hir::def_id::DefId;
use rustc::hir::Crate;
use rustc::lint::{LateContext, LateLintPass, LintArray, LintPass};
use rustc::mir;
use rustc::ty;
use rustc::ty::TyCtxt;
use rustc_data_structures::fx::FxHashSet;
use rustc_index::bit_set::BitSet;
use rustc_index::vec::Idx;
use rustc_mir::dataflow::{do_dataflow, BitDenotation, BottomValue, DataflowResultsCursor, DebugFormatted, GenKillSet};
use rustc_session::declare_tool_lint;
use syntax::source_map::Span;
use syntax_pos::symbol::Symbol;

declare_clippy_lint! {
    ///Checks whether the init/foo API is used correctly
    pub INIT_BEFORE_FOO,
    correctness,
    "must call the `init` function before the `foo` function"
}

declare_lint_pass!(Pass => [INIT_BEFORE_FOO]);

impl<'a, 'tcx> LateLintPass<'a, 'tcx> for Pass {
    #[allow(clippy::too_many_lines)]
    fn check_crate(&mut self, cx: &LateContext<'a, 'tcx>, _: &'tcx Crate) {
        // Only trigger the lint if this function has a main function
        if let Some(main_fn) = cx.tcx.get_diagnostic_item(Symbol::intern("check_main")) {
            #[allow(clippy::default_trait_access)]
            let mut call_stack: FxHashSet<DefId> = Default::default();
            if let InitState::NeedsInit(span) = check_init(cx.tcx, main_fn, &mut call_stack) {
                span_lint(
                    cx,
                    INIT_BEFORE_FOO,
                    span,
                    "call to `foo` not preceded by call to `init`",
                );
            }
        }
    }
}

enum InitState {
    Init,
    NotInit,
    NeedsInit(Vec<Span>),
}

fn check_init(tcx: TyCtxt<'_>, def_id: DefId, call_stack: &mut FxHashSet<DefId>) -> InitState {
    // Bail out on recursion (stack already contains a call to this function)
    if !call_stack.insert(def_id) {
        return InitState::NotInit;
    }
    let result = check_init_inner(tcx, def_id, call_stack);
    call_stack.remove(&def_id);
    result
}

fn check_init_inner(tcx: TyCtxt<'_>, def_id: DefId, call_stack: &mut FxHashSet<DefId>) -> InitState {
    if tcx.is_diagnostic_item(Symbol::intern("init"), def_id) {
        return InitState::Init;
    }

    // MIR from other crates may not be available, so we won't be able to detect anything there
    if !tcx.is_mir_available(def_id) {
        return InitState::NotInit;
    }

    // We just want the MIR of the function and don't care about any other information
    let mir = tcx.optimized_mir(def_id);

    let dead_unwinds = BitSet::new_empty(mir.basic_blocks().len());
    let seen_init = do_dataflow(
        tcx,
        mir,
        def_id,
        &[],
        &dead_unwinds,
        SeenInit {
            tcx,
            mir,
            call_stack: call_stack.clone(),
        },
        |_bd, _p| DebugFormatted::new(&"no id"),
    );
    let mut cursor = DataflowResultsCursor::new(seen_init, mir);

    for (block, bbdata) in mir.basic_blocks().iter_enumerated() {
        let terminator = bbdata.terminator();
        let callee_id = match &terminator.kind {
            mir::TerminatorKind::Call { func, .. } => match func.ty(&**mir, tcx).kind {
                ty::FnDef(def_id, _) => def_id,
                // Function pointer calls aren't implemented in this simple analyses, so we assume
                // any dynamic call to require init to have been called.
                _ => return InitState::NeedsInit(vec![terminator.source_info.span]),
            },
            // We only care about function calls
            _ => continue,
        };

        let loc = mir::Location {
            block,
            statement_index: bbdata.statements.len(),
        };
        // If init has not been called before reaching this source location,
        // then we must report an error on all `foo` calls encountered
        cursor.seek(loc);
        if !cursor.contains(NoIdx) && tcx.is_diagnostic_item(Symbol::intern("foo"), callee_id) {
            return InitState::NeedsInit(vec![terminator.source_info.span]);
        } else if let InitState::NeedsInit(mut span) = check_init(tcx, callee_id, call_stack) {
            span.push(terminator.source_info.span);
            return InitState::NeedsInit(span);
        }
    }
    InitState::NotInit
}

/// Determines whether `init` has been called at a specific point in the code
struct SeenInit<'a, 'tcx> {
    mir: &'a mir::Body<'tcx>,
    tcx: TyCtxt<'tcx>,
    call_stack: FxHashSet<DefId>,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
struct NoIdx;

impl Idx for NoIdx {
    fn index(self) -> usize {
        0
    }
    fn new(_: usize) -> Self {
        unimplemented!();
    }
}

impl<'a, 'tcx> BitDenotation<'tcx> for SeenInit<'a, 'tcx> {
    type Idx = NoIdx;
    fn name() -> &'static str {
        "seen init"
    }

    fn bits_per_block(&self) -> usize {
        1
    }

    fn start_block_effect(&self, _on_entry: &mut BitSet<NoIdx>) {}

    fn statement_effect(&self, _trans: &mut GenKillSet<NoIdx>, _loc: mir::Location) {}

    fn terminator_effect(&self, trans: &mut GenKillSet<NoIdx>, loc: mir::Location) {
        let func = match &self.mir[loc.block].terminator().kind {
            mir::TerminatorKind::Call { func, .. } => func,
            // We only care about function calls
            _ => return,
        };
        let callee_id = match func.ty(self.mir, self.tcx).kind {
            ty::FnDef(id, _) => id,
            // Function pointer calls aren't implemented in this simple analyses, so we assume
            // any dynamic call to require init to have been called.
            _ => return,
        };
        if let InitState::Init = check_init(self.tcx, callee_id, &mut self.call_stack.clone()) {
            trans.gen(NoIdx);
        }
    }

    fn propagate_call_return(
        &self,
        _in_out: &mut BitSet<NoIdx>,
        _call_bb: mir::BasicBlock,
        _dest_bb: mir::BasicBlock,
        _dest_place: &mir::Place<'tcx>,
    ) {
        // Nothing to do when a call returns successfully
    }
}

impl<'a, 'tcx> BottomValue for SeenInit<'a, 'tcx> {
    /// bottom = not seen
    const BOTTOM_VALUE: bool = false;
}
