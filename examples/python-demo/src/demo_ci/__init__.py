def greet(name: str) -> str:
    clean_name = name.strip() or "developer"
    return f"Hello {clean_name} from Python localci"
