use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};
use serde::Serialize;

pub(crate) fn conclusion(markdown: &str) -> Option<&str> {
    current_section(markdown).map(|s| markdown[s.body_start..s.end].trim())
}

pub(crate) fn current_section(markdown: &str) -> Option<SectionRange> {
    let mut sections = section_ranges(markdown).into_iter();
    let mut current = sections.find(|s| s.info.title == "结论")?;
    current.end = sections
        .find(|s| s.info.level <= current.info.level.max(2))
        .map(|s| s.start)
        .unwrap_or(markdown.len());
    Some(current)
}

pub(crate) fn excerpt(markdown: &str) -> String {
    let fallback_end = section_ranges(markdown)
        .into_iter()
        .find(|s| {
            matches!(
                s.info.title.as_str(),
                "更新日志" | "变更记录" | "此前结论" | "待采用建议" | "原文依据"
            )
        })
        .map(|s| s.start)
        .unwrap_or(markdown.len());
    let body = conclusion(markdown).unwrap_or(&markdown[..fallback_end]);
    body.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#') && !line.starts_with("<!--"))
        .take(3)
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches(['>', '*', '_', '`', ' '])
        .chars()
        .take(180)
        .collect()
}

pub(crate) fn retrieval_text(title: &str, markdown: &str, tags: &[String]) -> String {
    format!(
        "{title}\n{}\n{}",
        conclusion(markdown)
            .map(str::to_owned)
            .unwrap_or_else(|| excerpt(markdown)),
        tags.join(" ")
    )
}

pub(crate) fn replace_conclusion(markdown: &str, text: &str) -> String {
    let mut body = markdown.to_string();
    if let Some(section) = current_section(markdown) {
        body.replace_range(
            section.body_start..section.end,
            &format!("\n{}\n\n", text.trim()),
        );
    } else {
        body.insert_str(0, &format!("## 结论\n\n{}\n\n", text.trim()));
    }
    body
}

pub(crate) fn append_section(markdown: &str, title: &str, text: &str) -> String {
    let mut body = markdown.to_string();
    if let Some(section) = section_ranges(markdown)
        .into_iter()
        .find(|s| s.info.title == title && s.info.level == 2)
    {
        body.insert_str(section.end, &format!("\n\n{text}\n\n"));
    } else {
        body.push_str(&format!("\n\n## {title}\n\n{text}\n"));
    }
    body
}

pub(crate) fn record_change(markdown: &str, previous: &str, attribution: &str) -> String {
    append_section(
        markdown,
        "变更记录",
        &format!(
            "### {}\n\n{attribution}\n\n{}",
            chrono::Utc::now().to_rfc3339(),
            previous.trim()
        ),
    )
}

pub(crate) fn record_update(markdown: &str, event: &str) -> String {
    append_section(
        markdown,
        "更新日志",
        &format!(
            "- {} · {event}",
            chrono::Local::now().format("%Y-%m-%d %H:%M %:z")
        ),
    )
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    pub source_id: String,
    pub summary: String,
}

pub(crate) fn suggestions(markdown: &str) -> Vec<Suggestion> {
    let ranges = section_ranges(markdown);
    let Some(parent) = ranges
        .iter()
        .find(|s| s.info.title == "待采用建议" && s.info.level == 2)
    else {
        return Vec::new();
    };
    ranges
        .iter()
        .filter(|s| s.info.level == 3 && s.start > parent.start && s.end <= parent.end)
        .filter_map(|s| {
            let body = markdown[s.body_start..s.end].trim();
            let (marker, summary) = body.split_once('\n')?;
            Some(Suggestion {
                source_id: marker
                    .strip_prefix("<!-- suggestion:")?
                    .strip_suffix(" -->")?
                    .into(),
                summary: summary.trim().into(),
            })
        })
        .collect()
}

pub(crate) fn remove_suggestion(markdown: &str, source_id: &str) -> String {
    let marker = format!("<!-- suggestion:{source_id} -->");
    let mut body = markdown.to_string();
    if let Some(s) = section_ranges(markdown).into_iter().find(|s| {
        s.info.level == 3
            && markdown[s.body_start..s.end]
                .trim_start()
                .starts_with(&marker)
    }) {
        body.replace_range(s.start..s.end, "");
    }
    if let Some(section) = section_ranges(&body)
        .into_iter()
        .find(|s| s.info.title == "待采用建议" && s.info.level == 2)
    {
        if body[section.body_start..section.end].trim().is_empty() {
            body.replace_range(section.start..section.end, "");
        }
    }
    body
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceSource {
    pub source_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub message_role: Option<String>,
    pub status: String,
    pub start_line: Option<usize>,
    pub end_line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
}

pub(crate) fn evidence_sources(
    markdown: &str,
    sources: &[crate::storage::ArchiveSource],
) -> Vec<EvidenceSource> {
    let sections = section_ranges(markdown);
    let evidence = sections
        .iter()
        .filter(|s| s.info.title == "原文依据")
        .collect::<Vec<_>>();
    sources
        .iter()
        .enumerate()
        .map(|(index, source)| {
            let mut result = EvidenceSource {
                source_id: source.source_id.clone(),
                session_id: source.session_id.clone(),
                turn_id: source.turn_id.clone(),
                message_role: source.message_role.clone(),
                status: "unresolved".into(),
                start_line: None,
                end_line: None,
                quote: None,
            };
            let [section] = evidence.as_slice() else {
                return result;
            };
            let text = &markdown[section.body_start..section.end];
            let mut offset = 0;
            let mut markers = Vec::new();
            let mut legacy = Vec::new();
            for line in text.split_inclusive('\n') {
                if let Some(id) = line
                    .trim_end()
                    .strip_prefix("<!-- source:")
                    .and_then(|s| s.strip_suffix(" -->"))
                {
                    markers.push((offset, offset + line.len(), id));
                }
                if let Some(turn) = line
                    .trim()
                    .strip_prefix("**轮次 ")
                    .and_then(|s| s.strip_suffix("**"))
                {
                    legacy.push((offset, offset + line.len(), turn));
                }
                offset += line.len();
            }
            let matching = markers
                .iter()
                .enumerate()
                .filter(|(_, (_, _, id))| *id == source.source_id)
                .collect::<Vec<_>>();
            let range = if let [(position, (_, start, _))] = matching.as_slice() {
                Some((
                    *start,
                    markers.get(position + 1).map(|m| m.0).unwrap_or(text.len()),
                ))
            } else if matching.is_empty() {
                let legacy_end = markers.first().map(|m| m.0).unwrap_or(text.len());
                if index == 0 {
                    Some((
                        0,
                        legacy
                            .first()
                            .map(|m| m.0)
                            .unwrap_or(legacy_end)
                            .min(legacy_end),
                    ))
                } else if sources
                    .iter()
                    .filter(|s| s.turn_id == source.turn_id)
                    .count()
                    == 1
                {
                    let matching = legacy
                        .iter()
                        .enumerate()
                        .filter(|(_, m)| m.2 == source.turn_id && m.0 < legacy_end)
                        .collect::<Vec<_>>();
                    match matching.as_slice() {
                        [(position, (_, start, _))] => Some((
                            *start,
                            legacy
                                .get(position + 1)
                                .map(|m| m.0)
                                .unwrap_or(legacy_end)
                                .min(legacy_end),
                        )),
                        _ => None,
                    }
                } else {
                    None
                }
            } else {
                None
            };
            let Some((start, end)) = range else {
                return result;
            };
            let mut quotes = Vec::new();
            let mut quote_start = None;
            let mut quote_end = 0;
            let mut position = start;
            let mut finished = false;
            for line in text[start..end].split_inclusive('\n') {
                if let Some(quoted) = line.strip_suffix('\n').unwrap_or(line).strip_prefix('>') {
                    if finished {
                        return result;
                    }
                    quote_start.get_or_insert(position);
                    quote_end = position + line.len();
                    quotes.push(quoted.strip_prefix(' ').unwrap_or(quoted));
                } else if quote_start.is_some() {
                    finished = true;
                }
                position += line.len();
            }
            if let Some(start) = quote_start {
                result.status = "located".into();
                result.start_line = Some(
                    markdown[..section.body_start + start]
                        .bytes()
                        .filter(|b| *b == b'\n')
                        .count()
                        + 1,
                );
                result.end_line = Some(
                    markdown[..section.body_start + quote_end]
                        .trim_end_matches('\n')
                        .bytes()
                        .filter(|b| *b == b'\n')
                        .count()
                        + 1,
                );
                result.quote = Some(quotes.join("\n"));
            }
            result
        })
        .collect()
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SectionInfo {
    pub id: String,
    pub title: String,
    pub level: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expandable: Option<bool>,
}

pub(crate) struct SectionRange {
    pub(crate) info: SectionInfo,
    pub(crate) start: usize,
    pub(crate) body_start: usize,
    pub(crate) end: usize,
}

pub(crate) fn section_ranges(markdown: &str) -> Vec<SectionRange> {
    let mut sections = Vec::<SectionRange>::new();
    let mut heading = None::<(usize, u8, String)>;
    let mut container_depth = 0;
    for (event, range) in Parser::new(markdown).into_offset_iter() {
        match event {
            Event::Start(Tag::BlockQuote(_) | Tag::List(_)) => container_depth += 1,
            Event::End(TagEnd::BlockQuote(_) | TagEnd::List(_)) => container_depth -= 1,
            Event::Start(Tag::Heading { level, .. }) if container_depth == 0 => {
                heading = Some((range.start, heading_level(level), String::new()))
            }
            Event::Text(text) | Event::Code(text) => {
                if let Some((_, _, title)) = &mut heading {
                    title.push_str(&text);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some((_, _, title)) = &mut heading {
                    title.push(' ');
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((start, level, title)) = heading.take() {
                    sections.push(SectionRange {
                        info: SectionInfo {
                            id: format!("section-{}", sections.len() + 1),
                            title,
                            level,
                            source_count: None,
                            expandable: None,
                        },
                        start,
                        body_start: range.end,
                        end: markdown.len(),
                    });
                }
            }
            _ => {}
        }
    }
    for index in 0..sections.len() {
        sections[index].end = sections
            .iter()
            .skip(index + 1)
            .find(|next| next.info.level <= sections[index].info.level)
            .map(|next| next.start)
            .unwrap_or(markdown.len());
    }
    sections
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}
