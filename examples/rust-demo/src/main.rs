fn main() {
    let name = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "developer".to_string());
    println!("{}", rust_demo::greet(&name));
}
