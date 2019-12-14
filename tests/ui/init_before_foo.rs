#![feature(rustc_attrs)]

#[rustc_diagnostic_item = "check_main"]
fn main() {
    init();
    foo();
}

#[rustc_diagnostic_item = "foo"]
fn foo() {}

#[rustc_diagnostic_item = "init"]
fn init() {}
