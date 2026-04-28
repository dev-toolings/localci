pub fn greet(name: &str) -> String {
    let clean_name = name.trim();
    let name = if clean_name.is_empty() {
        "developer"
    } else {
        clean_name
    };

    format!("Hello {name} from Rust localci")
}

#[cfg(test)]
mod tests {
    use super::greet;

    #[test]
    fn greets_named_user() {
        assert_eq!(greet("Ada"), "Hello Ada from Rust localci");
    }

    #[test]
    fn greets_default_user() {
        assert_eq!(greet(""), "Hello developer from Rust localci");
    }
}
