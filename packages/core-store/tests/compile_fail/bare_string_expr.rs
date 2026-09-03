fn main() {
    // Runtime expressions must be PathComponent, not bare String/&str.
    let name = String::from("alice");
    let _ = structfs_core_store::path!("users", name);
}
