#![feature(rustc_attrs)]

#[rustc_diagnostic_item = "check_main"]
fn main() {
    if false_value() {
        init();
    }
    foo();
}

#[inline(never)]
fn false_value() -> bool {
    false
}

#[rustc_diagnostic_item = "foo"]
fn foo() {}

#[rustc_diagnostic_item = "init"]
fn init() {}
