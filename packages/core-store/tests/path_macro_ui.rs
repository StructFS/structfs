//! Compile-fail tests pinning the `path!` macro's compile-time validation.

#[test]
fn compile_fail() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/compile_fail/*.rs");
}
