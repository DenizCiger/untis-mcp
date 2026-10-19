use super::api::{RawClass, RawRoom, RawTeacher};
use crate::models::{TimetableSearchItem, TimetableSearchTargetType};
use std::cmp::Ordering;
use std::collections::HashMap;
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

pub fn search_timetable_targets(
    items: &[TimetableSearchItem],
    query: &str,
    limit: Option<usize>,
) -> Vec<TimetableSearchItem> {
    let normalized_query = normalize(query);
    if normalized_query.is_empty() {
        let mut all = items.to_vec();
        all.sort_by(compare_search_items);
        if let Some(limit) = limit {
            all.truncate(limit);
        }
        return all;
    }

    let tokens = tokenize(&normalized_query);
    let compact_query = normalized_query.replace(' ', "");
    let mut ranked = items
        .iter()
        .filter_map(|item| {
            let prepared = prepare_search_item(item);
            get_match_rank(&prepared, &normalized_query, &tokens, &compact_query)
                .map(|score| (item.clone(), score))
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        left.1
            .rank
            .cmp(&right.1.rank)
            .then_with(|| left.1.penalty.cmp(&right.1.penalty))
            .then_with(|| compare_search_items(&left.0, &right.0))
    });
    let mut results = ranked.into_iter().map(|entry| entry.0).collect::<Vec<_>>();
    if let Some(limit) = limit {
        results.truncate(limit);
    }
    results
}

pub(super) fn map_teachers_to_search_items(teachers: &[RawTeacher]) -> Vec<TimetableSearchItem> {
    teachers
        .iter()
        .map(|teacher| {
            let short = teacher.name.trim();
            let surname = teacher.long_name.trim();
            let forename = teacher.fore_name.trim();
            let combined = format!("{surname} {forename}").trim().to_owned();
            let display = if !combined.is_empty() {
                combined.clone()
            } else if !surname.is_empty() {
                surname.to_owned()
            } else if !short.is_empty() {
                short.to_owned()
            } else {
                teacher.id.to_string()
            };
            let secondary = if !short.is_empty() && short != display {
                short.to_owned()
            } else if !surname.is_empty() && surname != display {
                surname.to_owned()
            } else {
                display.clone()
            };
            TimetableSearchItem {
                r#type: TimetableSearchTargetType::Teacher,
                id: teacher.id,
                name: display.clone(),
                long_name: secondary.clone(),
                search_text: build_search_text(&[
                    display,
                    secondary,
                    short.to_owned(),
                    surname.to_owned(),
                    forename.to_owned(),
                ]),
            }
        })
        .collect()
}

pub(super) fn map_rooms_to_search_items(rooms: &[RawRoom]) -> Vec<TimetableSearchItem> {
    rooms
        .iter()
        .map(|room| TimetableSearchItem {
            r#type: TimetableSearchTargetType::Room,
            id: room.id,
            name: if room.name.is_empty() {
                if room.long_name.is_empty() {
                    room.id.to_string()
                } else {
                    room.long_name.clone()
                }
            } else {
                room.name.clone()
            },
            long_name: if room.long_name.is_empty() {
                if room.name.is_empty() {
                    room.id.to_string()
                } else {
                    room.name.clone()
                }
            } else {
                room.long_name.clone()
            },
            search_text: build_search_text(&[
                room.name.clone(),
                room.long_name.clone(),
                room.alternate_name.clone(),
            ]),
        })
        .collect()
}

pub(super) fn map_classes_to_search_items(classes: &[RawClass]) -> Vec<TimetableSearchItem> {
    classes
        .iter()
        .map(|class| TimetableSearchItem {
            r#type: TimetableSearchTargetType::Class,
            id: class.id,
            name: if class.name.is_empty() {
                if class.long_name.is_empty() {
                    class.id.to_string()
                } else {
                    class.long_name.clone()
                }
            } else {
                class.name.clone()
            },
            long_name: if class.long_name.is_empty() {
                if class.name.is_empty() {
                    class.id.to_string()
                } else {
                    class.name.clone()
                }
            } else {
                class.long_name.clone()
            },
            search_text: build_search_text(&[class.name.clone(), class.long_name.clone()]),
        })
        .collect()
}

fn build_search_text(parts: &[String]) -> String {
    parts
        .iter()
        .filter(|part| !part.trim().is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

pub(super) fn normalize_search_items(items: Vec<TimetableSearchItem>) -> Vec<TimetableSearchItem> {
    let mut deduped = HashMap::<(TimetableSearchTargetType, i64), TimetableSearchItem>::new();
    for item in items {
        deduped.entry((item.r#type, item.id)).or_insert(item);
    }
    let mut values: Vec<_> = deduped.into_values().collect();
    values.sort_by(compare_search_items);
    values
}

fn normalize(value: &str) -> String {
    value
        .nfkd()
        .filter(|character| !is_combining_mark(*character))
        .collect::<String>()
        .trim()
        .to_lowercase()
}

fn tokenize(value: &str) -> Vec<String> {
    normalize(value)
        .split_whitespace()
        .filter(|part| !part.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn to_words(value: &str) -> Vec<String> {
    let mut current = String::new();
    let mut words = Vec::new();
    for character in normalize(value).chars() {
        if character.is_ascii_alphanumeric() {
            current.push(character);
        } else if !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

fn compact(value: &str) -> String {
    normalize(value)
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect()
}

fn initials(words: &[String]) -> String {
    words
        .iter()
        .filter_map(|word| word.chars().next())
        .collect::<String>()
}

fn search_type_order(target_type: TimetableSearchTargetType) -> i32 {
    match target_type {
        TimetableSearchTargetType::Class => 0,
        TimetableSearchTargetType::Room => 1,
        TimetableSearchTargetType::Teacher => 2,
    }
}

fn compare_search_items(left: &TimetableSearchItem, right: &TimetableSearchItem) -> Ordering {
    search_type_order(left.r#type)
        .cmp(&search_type_order(right.r#type))
        .then_with(|| left.name.cmp(&right.name))
        .then_with(|| left.long_name.cmp(&right.long_name))
        .then_with(|| left.id.cmp(&right.id))
}

fn has_all_tokens(haystack: &str, tokens: &[String]) -> bool {
    tokens.iter().all(|token| haystack.contains(token))
}

fn token_contains_penalty(haystack: &str, tokens: &[String]) -> Option<usize> {
    let mut penalty = 0;
    for token in tokens {
        penalty += haystack.find(token)?;
    }
    Some(penalty)
}

fn word_prefix_penalty(words: &[String], tokens: &[String]) -> Option<usize> {
    let mut penalty = 0;
    for token in tokens {
        let mut best_word_idx = None;
        let mut best_length_delta = usize::MAX;
        for (index, word) in words.iter().enumerate() {
            if !word.starts_with(token) {
                continue;
            }
            let length_delta = word.len().saturating_sub(token.len());
            if length_delta < best_length_delta {
                best_word_idx = Some(index);
                best_length_delta = length_delta;
            }
        }
        penalty += best_word_idx? * 8 + best_length_delta;
    }
    Some(penalty)
}

fn fuzzy_subsequence_penalty(haystack: &str, query: &str) -> Option<usize> {
    if query.is_empty() {
        return Some(0);
    }
    let mut haystack_index = 0;
    let mut penalty = 0;
    let mut previous = None;
    for character in query.chars() {
        let next = haystack[haystack_index..].find(character)?;
        let absolute = haystack_index + next;
        penalty += previous
            .map(|prev| absolute.saturating_sub(prev + 1))
            .unwrap_or(absolute);
        previous = Some(absolute);
        haystack_index = absolute + character.len_utf8();
    }
    penalty += haystack.len().saturating_sub(haystack_index);
    Some(penalty)
}

fn bounded_edit_distance(left: &str, right: &str, max_distance: usize) -> Option<usize> {
    let left = left.chars().collect::<Vec<_>>();
    let right = right.chars().collect::<Vec<_>>();
    if left.len().abs_diff(right.len()) > max_distance {
        return None;
    }

    let mut previous = (0..=right.len()).collect::<Vec<_>>();
    for (left_index, left_char) in left.iter().enumerate() {
        let mut current = vec![left_index + 1];
        let mut row_min = current[0];
        for (right_index, right_char) in right.iter().enumerate() {
            let insertion = current[right_index] + 1;
            let deletion = previous[right_index + 1] + 1;
            let substitution = previous[right_index] + usize::from(left_char != right_char);
            let distance = insertion.min(deletion).min(substitution);
            row_min = row_min.min(distance);
            current.push(distance);
        }
        if row_min > max_distance {
            return None;
        }
        previous = current;
    }

    let distance = previous[right.len()];
    (distance <= max_distance).then_some(distance)
}

fn typo_tolerance(query: &str) -> usize {
    if query.chars().any(|character| character.is_ascii_digit()) {
        return 0;
    }
    match query.chars().count() {
        0..=3 => 0,
        4..=6 => 1,
        _ => 2,
    }
}

fn typo_penalty(candidates: &[String], query: &str) -> Option<usize> {
    let max_distance = typo_tolerance(query);
    if max_distance == 0 {
        return None;
    }
    candidates
        .iter()
        .filter(|candidate| !candidate.is_empty())
        .filter_map(|candidate| {
            bounded_edit_distance(candidate, query, max_distance)
                .map(|distance| distance * 24 + candidate.len().abs_diff(query.len()))
        })
        .min()
}

#[derive(Debug, Clone)]
struct PreparedSearchItem {
    name: String,
    long_name: String,
    search_text: String,
    compact_search: String,
    compact_name: String,
    compact_long_name: String,
    name_words: Vec<String>,
    long_name_words: Vec<String>,
    search_words: Vec<String>,
    initials: String,
}

#[derive(Debug, Clone, Copy)]
struct MatchRank {
    rank: usize,
    penalty: usize,
}

fn prepare_search_item(item: &TimetableSearchItem) -> PreparedSearchItem {
    let name = normalize(&item.name);
    let long_name = normalize(&item.long_name);
    let search_source = if item.search_text.is_empty() {
        format!("{} {}", item.name, item.long_name)
    } else {
        item.search_text.clone()
    };
    let search_text = normalize(&search_source);
    let name_words = to_words(&name);
    let long_name_words = to_words(&long_name);
    let search_words = to_words(&search_text);
    let initials = initials(&search_words);
    PreparedSearchItem {
        name: name.clone(),
        long_name: long_name.clone(),
        compact_search: compact(&search_text),
        compact_name: compact(&name),
        compact_long_name: compact(&long_name),
        search_text,
        name_words,
        long_name_words,
        search_words,
        initials,
    }
}

fn get_match_rank(
    item: &PreparedSearchItem,
    normalized_query: &str,
    tokens: &[String],
    compact_query: &str,
) -> Option<MatchRank> {
    if item.name.starts_with(normalized_query) {
        return Some(MatchRank {
            rank: 0,
            penalty: item.name.len().saturating_sub(normalized_query.len()),
        });
    }
    if item.long_name.starts_with(normalized_query) {
        return Some(MatchRank {
            rank: 1,
            penalty: item.long_name.len().saturating_sub(normalized_query.len()),
        });
    }
    if let Some(penalty) = word_prefix_penalty(&item.name_words, tokens) {
        return Some(MatchRank { rank: 2, penalty });
    }
    if let Some(penalty) = word_prefix_penalty(&item.long_name_words, tokens) {
        return Some(MatchRank { rank: 3, penalty });
    }
    if has_all_tokens(&item.name, tokens) {
        return Some(MatchRank {
            rank: 4,
            penalty: token_contains_penalty(&item.name, tokens).unwrap_or(0),
        });
    }
    if has_all_tokens(&item.long_name, tokens) {
        return Some(MatchRank {
            rank: 5,
            penalty: token_contains_penalty(&item.long_name, tokens).unwrap_or(0),
        });
    }
    if has_all_tokens(&item.search_text, tokens) {
        return Some(MatchRank {
            rank: 6,
            penalty: token_contains_penalty(&item.search_text, tokens).unwrap_or(0),
        });
    }
    if item.compact_name.starts_with(compact_query) {
        return Some(MatchRank {
            rank: 7,
            penalty: item.compact_name.len().saturating_sub(compact_query.len()),
        });
    }
    if item.compact_long_name.starts_with(compact_query) {
        return Some(MatchRank {
            rank: 8,
            penalty: item
                .compact_long_name
                .len()
                .saturating_sub(compact_query.len()),
        });
    }
    if item.compact_search.contains(compact_query) {
        return Some(MatchRank {
            rank: 9,
            penalty: item.compact_search.find(compact_query).unwrap_or(0),
        });
    }
    if item.initials.starts_with(compact_query) {
        return Some(MatchRank {
            rank: 10,
            penalty: item.initials.len().saturating_sub(compact_query.len()),
        });
    }
    if let Some(penalty) = fuzzy_subsequence_penalty(&item.initials, compact_query) {
        return Some(MatchRank { rank: 11, penalty });
    }
    if let Some(penalty) = typo_penalty(&item.search_words, compact_query) {
        return Some(MatchRank { rank: 12, penalty });
    }
    fuzzy_subsequence_penalty(&item.compact_search, compact_query)
        .map(|penalty| MatchRank { rank: 13, penalty })
}
