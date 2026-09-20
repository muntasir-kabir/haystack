//! Small, cursor-aware completion list for inline format placeholders.

use std::ops::Range;

pub struct Completion {
    pub replace: Range<usize>,
    pub choices: Vec<&'static str>,
}

const FIELDS: [&str; 6] = ["time", "log", "ignore", "thread", "process", "loglevel"];
const TYPES: [&str; 4] = ["token", "number", "path", "text"];

pub fn at_cursor(layout: &str, cursor_char: usize) -> Option<Completion> {
    let byte = layout
        .char_indices()
        .nth(cursor_char)
        .map_or(layout.len(), |(at, _)| at);
    let before = &layout[..byte];
    let open = before.rfind('{')?;
    if before[open + 1..].contains(['}', '{']) || before[..open].ends_with('{') {
        return None;
    }
    let declaration_end = layout[byte..]
        .find('}')
        .map_or(layout.len(), |offset| byte + offset);
    let declaration = &layout[open + 1..declaration_end];
    if declaration.contains('{') || declaration.matches(':').count() > 1 {
        return None;
    }
    let cursor_in_declaration = byte.saturating_sub(open + 1);
    let (replace, typed, choices): (Range<usize>, &str, &[&'static str]) =
        if let Some(colon) = declaration.find(':') {
            if cursor_in_declaration <= colon {
                let replace = open + 1..open + 1 + colon;
                (replace, &layout[open + 1..byte], &FIELDS)
            } else {
                let replace = open + 1 + colon + 1..declaration_end;
                (replace, &layout[open + 1 + colon + 1..byte], &TYPES)
            }
        } else {
            let replace = open + 1..declaration_end;
            (replace, &layout[open + 1..byte], &FIELDS)
        };
    let choices = choices
        .iter()
        .copied()
        .filter(|item| item.starts_with(typed))
        .collect();
    Some(Completion { replace, choices })
}

pub fn insert(layout: &mut String, completion: &Completion, choice: &str) -> usize {
    layout.replace_range(completion.replace.clone(), choice);
    let new_end = completion.replace.start + choice.len();
    if layout.as_bytes().get(new_end) != Some(&b'}') {
        layout.insert(new_end, '}');
    }
    // egui cursor indices are Unicode scalar values, not byte offsets.
    layout[..new_end + 1].chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completes_type_without_destroying_unicode_prefix_or_existing_brace() {
        let mut layout = "日志 {time} {worker:nu} {log}".to_string();
        let position = "日志 {time} {worker:nu".chars().count();
        let completion = at_cursor(&layout, position).unwrap();
        assert_eq!(completion.choices, vec!["number"]);
        let cursor = insert(&mut layout, &completion, "number");
        assert_eq!(layout, "日志 {time} {worker:number} {log}");
        assert_eq!(cursor, "日志 {time} {worker:number}".chars().count());
    }

    #[test]
    fn custom_names_are_allowed_without_completion() {
        let layout = "{time} {request_id";
        let completion = at_cursor(layout, layout.chars().count()).unwrap();
        assert!(completion.choices.is_empty());
    }

    #[test]
    fn completion_replaces_the_full_token_after_the_caret() {
        let mut layout = "{time} {proXcess} {log}".to_string();
        let position = "{time} {pro".chars().count();
        let completion = at_cursor(&layout, position).unwrap();
        assert_eq!(completion.choices, vec!["process"]);
        let cursor = insert(&mut layout, &completion, "process");
        assert_eq!(layout, "{time} {process} {log}");
        assert_eq!(cursor, "{time} {process}".chars().count());
    }

    #[test]
    fn completion_replaces_a_type_without_discarding_the_field_name() {
        let mut layout = "{time} {worker:nuMber} {log}".to_string();
        let position = "{time} {worker:nu".chars().count();
        let completion = at_cursor(&layout, position).unwrap();
        assert_eq!(completion.choices, vec!["number"]);
        insert(&mut layout, &completion, "number");
        assert_eq!(layout, "{time} {worker:number} {log}");
    }
}
