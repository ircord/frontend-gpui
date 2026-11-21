//! Guided argument entry, from the web client's
//! `shared/utils/command-arguments.ts`: which argument is being typed, and
//! whether the ones typed so far are valid. Values are never echoed back in
//! an error, so a channel key stays out of what is shown.

use super::catalog::{Field, Spec, resolve};

/// What is wrong with one argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgError {
    Required,
    /// Control characters.
    Control,
    /// Spaces in a one-word argument.
    Token,
    MaxBytes,
}

/// A command being typed, as guided entry shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Guide {
    pub spec: &'static Spec,
    /// The argument being typed now, if the command has one left.
    pub current: Option<usize>,
    /// The values typed so far, one per field.
    pub values: Vec<String>,
    /// The first argument typed so far that is not valid, and why.
    pub error: Option<(usize, ArgError)>,
}

impl Guide {
    pub fn field(&self) -> Option<&'static Field> {
        self.current.map(|i| &self.spec.fields[i])
    }
}

/// The guide for `input` when it is a known command with arguments and a
/// space after its name: `/kick bo` is on the first argument.
pub fn guide(input: &str) -> Option<Guide> {
    let rest = input.strip_prefix('/')?;
    if rest.starts_with('/') {
        return None;
    }
    let (name, args) = rest.split_once(char::is_whitespace)?;
    let spec = resolve(name)?;
    if spec.fields.is_empty() {
        return None;
    }
    let (values, current) = split(spec, args);
    let error = spec.fields.iter().enumerate().find_map(|(i, field)| {
        let value = values.get(i).map_or("", String::as_str);
        // Only fields already passed can be missing.
        let passed = current.is_none_or(|c| i < c);
        check(field, value, passed).map(|e| (i, e))
    });
    Some(Guide {
        spec,
        current,
        values,
        error,
    })
}

/// Split typed arguments into the fields they fill: a one-word field takes
/// the next word, any other field the rest of the line. Also returns the
/// field the end of the text is in.
fn split(spec: &Spec, args: &str) -> (Vec<String>, Option<usize>) {
    let mut values = Vec::new();
    let mut rest = args.trim_start();
    let mut current = Some(0);
    for (i, field) in spec.fields.iter().enumerate() {
        current = Some(i);
        if !field.token {
            values.push(rest.to_string());
            break;
        }
        match rest.split_once(char::is_whitespace) {
            Some((word, after)) => {
                values.push(word.to_string());
                rest = after.trim_start();
                current = Some(i + 1);
            }
            None => {
                values.push(rest.to_string());
                break;
            }
        }
    }
    if current.is_some_and(|c| c >= spec.fields.len()) {
        current = None;
    }
    (values, current)
}

fn check(field: &Field, value: &str, passed: bool) -> Option<ArgError> {
    if value.chars().any(char::is_control) {
        return Some(ArgError::Control);
    }
    if field.required && passed && value.trim().is_empty() {
        return Some(ArgError::Required);
    }
    if field.token && value.contains(char::is_whitespace) {
        return Some(ArgError::Token);
    }
    if field.max_bytes.is_some_and(|max| value.len() > max) {
        return Some(ArgError::MaxBytes);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_the_argument_being_typed() {
        let g = guide("/kick ").unwrap();
        assert_eq!(g.spec.name, "kick");
        assert_eq!(g.field().unwrap().name, "nickname");
        let g = guide("/kick bob").unwrap();
        assert_eq!(g.current, Some(0));
        assert_eq!(g.values, ["bob"]);
        let g = guide("/kick bob being rude").unwrap();
        assert_eq!(g.field().unwrap().name, "reason");
        assert_eq!(g.values, ["bob", "being rude"]);
        let g = guide("/j #a key").unwrap();
        assert_eq!(g.spec.name, "join");
        assert_eq!(g.field().unwrap().name, "key");
        let g = guide("/j #a key ").unwrap();
        assert_eq!(g.current, None);
    }

    #[test]
    fn only_commands_with_arguments_and_a_space_are_guided() {
        assert!(guide("/kick").is_none());
        assert!(guide("/back ").is_none());
        assert!(guide("//kick bob").is_none());
        assert!(guide("hello /kick bob").is_none());
        assert!(guide("/nope x").is_none());
    }

    #[test]
    fn reports_invalid_arguments_without_their_values() {
        assert_eq!(guide("/kick ").unwrap().error, None);
        let long = format!("/status x {}", "y".repeat(201));
        assert_eq!(guide(&long).unwrap().error, Some((1, ArgError::MaxBytes)));
        assert_eq!(
            guide("/msg bob \u{7}").unwrap().error,
            Some((1, ArgError::Control))
        );
    }
}
