use secrecy::SecretString;
use std::str::FromStr;

pub struct EnvReader<F: Fn(&str) -> Option<String>> {
    get: F,
    pub errors: Vec<String>,
}

impl EnvReader<fn(&str) -> Option<String>> {
    pub fn from_process() -> Self {
        EnvReader::new(|k: &str| std::env::var(k).ok())
    }
}

impl<F: Fn(&str) -> Option<String>> EnvReader<F> {
    pub fn new(get: F) -> Self {
        Self { get, errors: Vec::new() }
    }

    fn raw(&self, name: &str) -> Option<String> {
        (self.get)(name).filter(|v| !v.is_empty())
    }

    pub fn optional(&mut self, name: &str) -> Option<String> {
        if let Some(v) = self.raw(name) {
            return Some(v);
        }
        let file_var = format!("{name}_FILE");
        let path = self.raw(&file_var)?;
        match std::fs::read_to_string(&path) {
            Ok(s) => Some(s.trim_end_matches(['\r', '\n']).to_string()).filter(|v| !v.is_empty()),
            Err(e) => {
                self.errors.push(format!("{file_var}: cannot read {path}: {e}"));
                None
            }
        }
    }

    pub fn required(&mut self, name: &str) -> Option<String> {
        let errors_before = self.errors.len();
        let value = self.optional(name);
        if value.is_none() && self.errors.len() == errors_before {
            self.errors.push(format!("{name} is required"));
        }
        value
    }

    pub fn secret_optional(&mut self, name: &str) -> Option<SecretString> {
        self.optional(name).map(SecretString::from)
    }

    pub fn secret_required(&mut self, name: &str) -> Option<SecretString> {
        self.required(name).map(SecretString::from)
    }

    pub fn bool(&mut self, name: &str, default: bool) -> bool {
        let Some(v) = self.optional(name) else { return default };
        match v.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => true,
            "0" | "false" | "no" | "off" => false,
            _ => {
                self.errors.push(format!("{name} must be true or false, got \"{v}\""));
                default
            }
        }
    }

    pub fn parse<T: FromStr>(&mut self, name: &str, default: T) -> T {
        self.parse_optional(name).unwrap_or(default)
    }

    pub fn parse_optional<T: FromStr>(&mut self, name: &str) -> Option<T> {
        let v = self.optional(name)?;
        match v.parse() {
            Ok(t) => Some(t),
            Err(_) => {
                self.errors.push(format!("{name} has an invalid value"));
                None
            }
        }
    }

    pub fn list(&mut self, name: &str) -> Vec<String> {
        self.optional(name)
            .map(|v| v.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;
    use std::collections::HashMap;

    fn reader(pairs: &[(&str, &str)]) -> EnvReader<impl Fn(&str) -> Option<String> + use<>> {
        let map: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        EnvReader::new(move |k: &str| map.get(k).cloned())
    }

    #[test]
    fn required_reports_missing() {
        let mut env = reader(&[]);
        assert_eq!(env.required("FOO"), None);
        assert_eq!(env.errors, vec!["FOO is required".to_string()]);
    }

    #[test]
    fn empty_values_count_as_unset() {
        let mut env = reader(&[("FOO", "")]);
        assert_eq!(env.optional("FOO"), None);
    }

    #[test]
    fn file_variant_is_read_and_trailing_newline_trimmed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret");
        std::fs::write(&path, "s3cret\n").unwrap();
        let p = path.to_str().unwrap().to_string();
        let mut env = reader(&[("PASS_FILE", p.as_str())]);
        assert_eq!(env.secret_required("PASS").unwrap().expose_secret(), "s3cret");
        assert!(env.errors.is_empty());
    }

    #[test]
    fn direct_value_wins_over_file() {
        let mut env = reader(&[("PASS", "direct"), ("PASS_FILE", "/nonexistent")]);
        assert_eq!(env.required("PASS").as_deref(), Some("direct"));
    }

    #[test]
    fn unreadable_file_is_an_error_without_leaking_contents() {
        let mut env = reader(&[("PASS_FILE", "/definitely/not/here")]);
        assert_eq!(env.required("PASS"), None);
        assert_eq!(env.errors.len(), 1);
        assert!(env.errors[0].starts_with("PASS_FILE: cannot read /definitely/not/here"));
    }

    #[test]
    fn bool_parsing() {
        let mut env = reader(&[("A", "true"), ("B", "0"), ("C", "yes"), ("D", "maybe")]);
        assert!(env.bool("A", false));
        assert!(!env.bool("B", true));
        assert!(env.bool("C", false));
        assert!(env.bool("MISSING", true));
        assert!(!env.bool("D", false));
        assert_eq!(env.errors, vec!["D must be true or false, got \"maybe\"".to_string()]);
    }

    #[test]
    fn parse_with_default_and_error() {
        let mut env = reader(&[("N", "42"), ("BAD", "x")]);
        assert_eq!(env.parse("N", 1u32), 42);
        assert_eq!(env.parse("MISSING", 7u32), 7);
        assert_eq!(env.parse("BAD", 3u32), 3);
        assert_eq!(env.errors, vec!["BAD has an invalid value".to_string()]);
    }

    #[test]
    fn list_splits_and_trims() {
        let mut env = reader(&[("L", " a, b ,,c ")]);
        assert_eq!(env.list("L"), vec!["a", "b", "c"]);
    }
}
