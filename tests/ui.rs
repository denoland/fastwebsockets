#[test]
fn ui() {
  let t = trybuild::TestCases::new();
  t.pass("tests/ui/01-send-executor.rs");
  t.pass("tests/ui/02-!send-executor.rs");
}
