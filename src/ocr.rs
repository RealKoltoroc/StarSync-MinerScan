#[derive(Debug, Clone)]
pub struct OcrResult {
    pub raw_text: String,
    pub numeric_value: Option<u32>,
    pub confidence: f32,
}

pub trait OcrEngine {
    fn recognize(&mut self, rgba: &[u8], width: u32, height: u32) -> anyhow::Result<OcrResult>;
}

#[derive(Default)]
pub struct NullOcrEngine;

impl OcrEngine for NullOcrEngine {
    fn recognize(&mut self, _rgba: &[u8], _width: u32, _height: u32) -> anyhow::Result<OcrResult> {
        Ok(OcrResult {
            raw_text: String::new(),
            numeric_value: None,
            confidence: 0.0,
        })
    }
}

pub fn normalize_numeric(text: &str) -> Option<u32> {
    let normalized: String = text.chars().filter_map(normalize_numeric_char).collect();

    if normalized.is_empty() {
        None
    } else {
        normalized.parse().ok()
    }
}

/// Extract likely RS values from a complete OCR text block without concatenating
/// unrelated text. Candidates must start with a real digit and contain at least
/// three real digits. Common OCR confusions are accepted only inside an already
/// numeric-looking token.
pub fn extract_numeric_candidates(text: &str) -> Vec<u32> {
    let mut values = Vec::new();

    for line in text.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        let mut index = 0usize;

        while index < parts.len() {
            let current = clean_numeric_token(parts[index]);

            // OCR/HUD rendering can split a thousands-formatted value as "7 200".
            // Only combine when the leading group has 1-3 digits and the following
            // group has exactly 3 digits, so independent values stay independent.
            if let Some(ref left) = current {
                if (1..=3).contains(&left.len()) && index + 1 < parts.len() {
                    if let Some(right) = clean_numeric_token(parts[index + 1]) {
                        if right.len() == 3 {
                            if let Ok(value) = format!("{left}{right}").parse::<u32>() {
                                values.push(value);
                                index += 2;
                                continue;
                            }
                        }
                    }
                }
            }

            if let Some(token) = current {
                if token.len() >= 3 {
                    if let Ok(value) = token.parse::<u32>() {
                        if value >= 100 {
                            values.push(value);
                        }
                    }
                }
            }

            index += 1;
        }
    }

    values.sort_unstable();
    values.dedup();
    values
}

fn clean_numeric_token(token: &str) -> Option<String> {
    let trimmed = token.trim_matches(|c: char| {
        matches!(
            c,
            ',' | '.' | ':' | ';' | '(' | ')' | '[' | ']' | '{' | '}' | '\'' | '’'
        )
    });
    if trimmed.is_empty() || !trimmed.chars().next()?.is_ascii_digit() {
        return None;
    }

    let mut normalized = String::new();
    let mut real_digits = 0usize;
    for c in trimmed.chars() {
        if c.is_ascii_digit() {
            normalized.push(c);
            real_digits += 1;
        } else if matches!(c, 'O' | 'o' | 'I' | 'l' | '|' | 'S' | 's' | 'B') {
            normalized.push(normalize_numeric_char(c)?);
        } else if matches!(c, ',' | '.' | '\'' | '’') {
            continue;
        } else {
            return None;
        }
    }

    (real_digits >= 1 && !normalized.is_empty()).then_some(normalized)
}

fn normalize_numeric_char(c: char) -> Option<char> {
    match c {
        '0'..='9' => Some(c),
        'O' | 'o' => Some('0'),
        'I' | 'l' | '|' => Some('1'),
        'S' | 's' => Some('5'),
        'B' => Some('8'),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_common_ocr_confusions() {
        assert_eq!(normalize_numeric("13S4O"), Some(13_540));
    }

    #[test]
    fn extracts_value_from_surrounding_text() {
        let text = "starsync-minerscan.d\n3385\nE starsync-minerscan.exe";
        assert_eq!(extract_numeric_candidates(text), vec![3385]);
    }

    #[test]
    fn extracts_spaced_scanner_value() {
        assert_eq!(extract_numeric_candidates("RS 7 200"), vec![7200]);
    }

    #[test]
    fn keeps_multiple_values_separate() {
        assert_eq!(
            extract_numeric_candidates("3385 7200 13540"),
            vec![3385, 7200, 13540]
        );
    }
}
