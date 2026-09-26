use structfs_core_store::path;
struct PretendComponent;
impl PretendComponent {
    #[allow(dead_code)]
    fn as_str(&self) -> &str { "bad-name" }
}
fn main() {
    let _ = path!("safe", PretendComponent);
}
