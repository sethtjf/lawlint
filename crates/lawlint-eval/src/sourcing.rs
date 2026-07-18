//! Shared, conservative cleaning and segmentation for public legal text.

use std::collections::HashSet;

pub fn normalize_whitespace(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !is_noise_line(line))
        .map(strip_footnote_markers)
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

fn strip_footnote_markers(line: &str) -> String {
    let mut output = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '[' {
            let mut marker = String::new();
            while let Some(next) = chars.peek().copied() {
                chars.next();
                if next == ']' {
                    break;
                }
                marker.push(next);
            }
            if !marker.is_empty()
                && marker.chars().all(|value| {
                    value.is_ascii_digit() || value.is_ascii_alphabetic() || value == ','
                })
            {
                continue;
            }
            output.push('[');
            output.push_str(&marker);
            output.push(']');
        } else if character.is_ascii_digit() && output.ends_with(' ') {
            // Common superscript footnote markers arrive as a separated digit.
            let mut lookahead = chars.clone();
            if lookahead.peek().is_none() {
                continue;
            }
            output.push(character);
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
    let mut entity = String::new();
    for character in html.chars() {
        if character == '<' {
            in_tag = true;
            text.push(' ');
        } else if character == '>' {
            in_tag = false;
            text.push(' ');
        } else if character == '&' && !in_tag {
            entity.clear();
            entity.push(character);
        } else if !entity.is_empty() && !in_tag {
            entity.push(character);
            if character == ';' {
                text.push(match entity.as_str() {
                    "&amp;" => '&',
                    "&lt;" => '<',
                    "&gt;" => '>',
                    "&quot;" => '"',
                    "&#39;" | "&apos;" => '\'',
                    _ => ' ',
                });
                entity.clear();
            }
        } else if !in_tag {
            text.push(character);
        }
    }
    normalize_whitespace(&text)
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
    let sentences = cleaned
        .split_inclusive(['.', '!', '?'])
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
