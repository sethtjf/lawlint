//! Shared, conservative cleaning and segmentation for public legal text.

use std::collections::HashSet;

pub fn normalize_whitespace(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !is_noise_line(line))
        .map(strip_standalone_footnote_markers)
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_noise_line(line: &str) -> bool {
    let trimmed = line.trim_matches(|character: char| {
        character.is_ascii_punctuation() || character.is_ascii_whitespace()
    });
    if trimmed.is_empty() {
        return true;
    }
    if trimmed.chars().all(|character| character.is_ascii_digit()) {
        return true;
    }
    let words = trimmed.split_whitespace().collect::<Vec<_>>();
    if words.len() <= 5
        && words.iter().all(|word| {
            word.chars()
                .all(|character| character.is_ascii_digit() || ".-–—".contains(character))
        })
    {
        return true;
    }
    let upper = trimmed.to_ascii_uppercase();
    upper.starts_with("PAGE ")
        || upper.starts_with("TABLE OF CONTENTS")
        || upper == "OPINION"
        || upper == "UNITED STATES REPORTS"
}

fn strip_standalone_footnote_markers(line: &str) -> String {
    let mut output = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '[' {
            let mut marker = String::new();
            let mut closed = false;
            while let Some(next) = chars.peek().copied() {
                chars.next();
                if next == ']' {
                    closed = true;
                    break;
                }
                marker.push(next);
            }
            if closed
                && !marker.is_empty()
                && marker.chars().all(|value| value.is_ascii_digit())
                && output.chars().last().is_some_and(char::is_whitespace)
            {
                continue;
            }
            output.push('[');
            output.push_str(&marker);
            if closed {
                output.push(']');
            }
        } else {
            output.push(character);
        }
    }
    output
}

pub fn strip_html(html: &str) -> String {
    let mut source = html.to_string();
    for tag in ["script", "style", "nav", "header", "footer"] {
        loop {
            let lower = source.to_ascii_lowercase();
            let Some(start) = lower.find(&format!("<{tag}")) else {
                break;
            };
            let Some(end_offset) = lower[start..].find(&format!("</{tag}>")) else {
                source.replace_range(start.., "");
                break;
            };
            source.replace_range(start..start + end_offset + tag.len() + 3, "");
        }
    }
    let html = source.as_str();
    let mut text = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut tag = String::new();
    let mut entity = String::new();
    for character in html.chars() {
        if character == '<' {
            in_tag = true;
            tag.clear();
        } else if character == '>' {
            in_tag = false;
            if is_block_tag(&tag) {
                text.push(' ');
            }
        } else if in_tag {
            tag.push(character);
        } else if character == '&' && !in_tag {
            entity.clear();
            entity.push(character);
        } else if !entity.is_empty() && !in_tag {
            entity.push(character);
            if character == ';' {
                if let Some(decoded) = decode_entity(&entity) {
                    text.push(decoded);
                } else {
                    text.push(' ');
                }
                entity.clear();
            }
        } else if !in_tag {
            text.push(character);
        }
    }
    normalize_whitespace(&text)
}

fn decode_entity(entity: &str) -> Option<char> {
    match entity {
        "&amp;" => Some('&'),
        "&lt;" => Some('<'),
        "&gt;" => Some('>'),
        "&quot;" => Some('"'),
        "&#39;" | "&#x27;" | "&#X27;" | "&apos;" => Some('\''),
        "&nbsp;" | "&#160;" | "&#xA0;" | "&#XA0;" => Some(' '),
        "&ndash;" => Some('–'),
        "&mdash;" => Some('—'),
        "&sect;" => Some('§'),
        "&hellip;" => Some('…'),
        _ if entity.starts_with("&#x") || entity.starts_with("&#X") => {
            char::from_u32(u32::from_str_radix(&entity[3..entity.len() - 1], 16).ok()?)
        }
        _ if entity.starts_with("&#") => char::from_u32(entity[2..entity.len() - 1].parse().ok()?),
        _ => None,
    }
}

fn is_block_tag(tag: &str) -> bool {
    let name = tag
        .trim_start_matches('/')
        .split_ascii_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        name.as_str(),
        "address"
            | "article"
            | "blockquote"
            | "br"
            | "div"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "li"
            | "p"
            | "pre"
            | "section"
            | "table"
            | "tr"
    )
}

pub fn trim_contract_preamble(text: &str) -> String {
    let markers = [
        "THIS AGREEMENT",
        "EMPLOYMENT AGREEMENT",
        "SERVICE AGREEMENT",
        "PURCHASE AGREEMENT",
        "MASTER AGREEMENT",
        "EXECUTION VERSION",
        "Dear ",
        "This ",
        "January ",
        "February ",
        "March ",
        "April ",
        "May ",
        "June ",
        "July ",
        "August ",
        "September ",
        "October ",
        "November ",
        "December ",
        "WHEREAS",
    ];
    let text = text.trim_start_matches("htm ").trim_start();
    markers
        .iter()
        .filter_map(|marker| text.find(marker))
        .min()
        .map_or_else(|| text.to_string(), |index| text[index..].to_string())
}

pub fn segment(text: &str, min_words: usize, max_words: usize) -> Vec<String> {
    let cleaned = normalize_whitespace(text);
    let sentences = split_sentences(&cleaned)
        .into_iter()
        .map(str::trim)
        .filter(|sentence| sentence.split_whitespace().count() >= 3);
    let mut passages = Vec::new();
    let mut current = Vec::new();
    for sentence in sentences {
        let sentence_words = sentence.split_whitespace().count();
        if !current.is_empty() && current.len() + sentence_words > max_words {
            if current.len() >= min_words {
                passages.push(current.join(" "));
            }
            current.clear();
        }
        current.extend(sentence.split_whitespace().map(str::to_string));
    }
    if current.len() >= min_words {
        passages.push(current.join(" "));
    }
    passages
}

fn split_sentences(text: &str) -> Vec<&str> {
    let mut sentences = Vec::new();
    let mut start = 0;
    for (index, character) in text.char_indices() {
        if !matches!(character, '.' | '!' | '?') {
            continue;
        }
        let after = index + character.len_utf8();
        let next = text[after..].chars().next();
        if next.is_some_and(|value| !value.is_whitespace()) {
            continue;
        }
        if character == '.' && is_abbreviation(text, start, index + 1) {
            continue;
        }
        let end = after;
        sentences.push(&text[start..end]);
        start = end;
    }
    if start < text.len() {
        sentences.push(&text[start..]);
    }
    sentences
}

fn is_abbreviation(text: &str, sentence_start: usize, period_end: usize) -> bool {
    let token_start = text[..period_end - 1]
        .char_indices()
        .rev()
        .find(|(_, character)| character.is_whitespace())
        .map_or(sentence_start, |(index, _)| index + 1);
    let token = &text[token_start..period_end];
    let bare = token.trim_matches(|character: char| !character.is_ascii_alphabetic());
    token[..token.len() - 1].contains('.')
        || bare.len() <= 3
        || matches!(
            bare.to_ascii_lowercase().as_str(),
            "ct" | "cir" | "co" | "inc" | "l" | "no" | "rev" | "stat"
        )
}

pub fn deduplicate(passages: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    passages
        .into_iter()
        .filter(|passage| {
            let normalized = passage
                .split_whitespace()
                .take(35)
                .collect::<Vec<_>>()
                .join(" ")
                .to_ascii_lowercase();
            seen.insert(normalized)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{normalize_whitespace, segment, strip_html};

    #[test]
    fn preserves_abbreviations_and_inline_numbers() {
        let text =
            "The U.S. Supreme Court cited S. Ct. and L. Ed. 2d at 0.120 ppm. See 62 Stat. 30.";
        assert_eq!(normalize_whitespace(text), text);
    }

    #[test]
    fn sentence_segmentation_does_not_split_abbreviations() {
        let text = "The U.S. Supreme Court cited 514 U.S. 73, 84, 115 S. Ct. 1223, 131 L. Ed. 2d 94 (1995). This sentence follows.";
        let passages = segment(text, 1, 100);
        assert_eq!(passages, vec![text.to_string()]);
    }

    #[test]
    fn html_inline_tags_do_not_split_words() {
        assert_eq!(
            strip_html("<p>U.S. <i>Supreme</i> Court cited 62 <span>Stat.</span> 30.</p>"),
            "U.S. Supreme Court cited 62 Stat. 30."
        );
    }
}
