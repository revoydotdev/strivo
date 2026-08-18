//! REFI-QDA Project (`.qdpx`) interchange export.
//!
//! Spec source: *REFI-QDA Standard*, v1.5 (2019-09-25), Fred van Blommestein
//! (ed.), Rotterdam Exchange Format Initiative,
//! <https://openqda.github.io/refi-tools/docs/standard/REFI-QDA-1-5.pdf>
//! (fetched 2026-08-18; downloaded to verify element/attribute names against
//! the standard's own §5.3 XML Schema listing rather than reconstructing it
//! from memory or from vendor documentation). The Project XML Schema's
//! `targetNamespace` and canonical schema location are taken verbatim from
//! that document (§5.3, `ProjectType`/`SourcesType`/`CodeType` etc.):
//! `urn:QDA-XML:project:1.0`,
//! `http://schema.qdasoftware.org/versions/Project/v1.0/Project.xsd`.
//!
//! This module emits the `<Project>` root element (not the `.qdpx` zip
//! container — no external binary payloads are packaged, only the XML
//! interchange document itself) with:
//! - `<Users>` — one `<User>` per distinct coding/memo/relationship author,
//!   with a schema-valid GUID derived deterministically from the author
//!   name (`crate::stable_id`) so re-exports are stable.
//! - `<CodeBook>` — the full `<Codes>` hierarchy (`CodeType` is recursive
//!   per the schema: a `<Code>` nests child `<Code>` elements), matching
//!   `codes.parent_id`.
//! - `<Sources>` — one element per `sources` row with its human codings
//!   nested as time-ranged selections.
//!
//! **Source-type mapping decision.** The REFI-QDA schema only defines
//! time-range addressing (`begin`/`end` in milliseconds) for
//! `AudioSelection` and `VideoSelection`; `TextSource` uses character
//! offsets and `PictureSource`/`PDFSource` use spatial coordinates — none
//! of which this crate's `signals`/`codings` schema produces (everything is
//! `start_ms`/`end_ms`). So every `sources.kind = 'audio'` row exports as
//! `<AudioSource>`, and every other kind (`recording`, `video`, `chat`,
//! `document`, `dataset`) exports as `<VideoSource>` — the closest
//! schema-valid element that supports millisecond time ranges. This is a
//! deliberate, documented simplification: a chat log or dataset source
//! becomes a schema-correct `<VideoSource>` with no actual video, but its
//! coded time ranges survive the round trip losslessly. Cases, variables,
//! notes, links, sets, and graphs are not exported (all `minOccurs="0"` in
//! the schema, so a document omitting them is still spec-valid) — this is a
//! partial but spec-correct export, not an invented one.
//!
//! Only `codings.origin = 'human'` rows are exported as `<Coding>`
//! elements: REFI-QDA's `CodingType` has no field for coding provenance, so
//! model-assisted or imported codings would be indistinguishable from human
//! judgements on import into NVivo/ATLAS.ti/MAXQDA — exporting them as
//! plain `<Coding>` would misrepresent their origin to the receiving tool.

use std::collections::HashMap;
use std::fmt::Write as _;

use uuid::Uuid;

use crate::{stable_id, Code, Coding, ResearchStore, Result};

const PROJECT_NAMESPACE: &str = "urn:QDA-XML:project:1.0";
const PROJECT_SCHEMA_LOCATION: &str =
    "http://schema.qdasoftware.org/versions/Project/v1.0/Project.xsd";

impl ResearchStore {
    /// Export `project_id` as a REFI-QDA Project XML document (`<Project>`
    /// root, `urn:QDA-XML:project:1.0`). See the module doc comment for the
    /// spec source and the documented source-type/provenance mapping.
    pub fn export_refi(&self, project_id: Uuid) -> Result<String> {
        let project = self
            .list_projects()?
            .into_iter()
            .find(|project| project.id == project_id)
            .ok_or_else(|| crate::ResearchError::Validation("project not found".into()))?;
        let codes = self.list_codes(project_id)?;
        let sources = self.list_sources(project_id)?;
        let codings = self.list_codings(project_id)?;

        let mut codings_by_source: HashMap<Uuid, Vec<&Coding>> = HashMap::new();
        let mut authors: Vec<String> = Vec::new();
        for coding in &codings {
            if coding.origin != "human" {
                continue;
            }
            codings_by_source
                .entry(coding.source_id)
                .or_default()
                .push(coding);
            if !authors.contains(&coding.author) {
                authors.push(coding.author.clone());
            }
        }
        authors.sort();

        let mut xml = String::new();
        xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        let _ = writeln!(
            xml,
            "<Project xmlns=\"{ns}\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" \
             xsi:schemaLocation=\"{ns} {loc}\" guid=\"{guid}\" name=\"{name}\">",
            ns = PROJECT_NAMESPACE,
            loc = PROJECT_SCHEMA_LOCATION,
            guid = project.id,
            name = escape_attr(&project.name),
        );

        write_users(&mut xml, &authors);
        write_codebook(&mut xml, &codes);
        write_sources(&mut xml, &sources, &codings_by_source);

        xml.push_str("</Project>\n");
        Ok(xml)
    }
}

fn write_users(xml: &mut String, authors: &[String]) {
    if authors.is_empty() {
        return;
    }
    xml.push_str("  <Users>\n");
    for author in authors {
        let _ = writeln!(
            xml,
            "    <User guid=\"{guid}\" name=\"{name}\"/>",
            guid = author_guid(author),
            name = escape_attr(author),
        );
    }
    xml.push_str("  </Users>\n");
}

fn write_codebook(xml: &mut String, codes: &[Code]) {
    xml.push_str("  <CodeBook>\n    <Codes>\n");
    for root in codes.iter().filter(|code| code.parent_id.is_none()) {
        write_code(xml, root, codes, 3);
    }
    xml.push_str("    </Codes>\n  </CodeBook>\n");
}

fn write_code(xml: &mut String, code: &Code, all_codes: &[Code], depth: usize) {
    let indent = "  ".repeat(depth);
    let children: Vec<&Code> = all_codes
        .iter()
        .filter(|candidate| candidate.parent_id == Some(code.id))
        .collect();
    if children.is_empty() {
        let _ = writeln!(
            xml,
            "{indent}<Code guid=\"{guid}\" name=\"{name}\" isCodable=\"true\" color=\"{color}\"/>",
            guid = code.id,
            name = escape_attr(&code.name),
            color = escape_attr(&code.color),
        );
        return;
    }
    let _ = writeln!(
        xml,
        "{indent}<Code guid=\"{guid}\" name=\"{name}\" isCodable=\"true\" color=\"{color}\">",
        guid = code.id,
        name = escape_attr(&code.name),
        color = escape_attr(&code.color),
    );
    for child in children {
        write_code(xml, child, all_codes, depth + 1);
    }
    let _ = writeln!(xml, "{indent}</Code>");
}

fn write_sources(
    xml: &mut String,
    sources: &[crate::Source],
    codings_by_source: &HashMap<Uuid, Vec<&Coding>>,
) {
    if sources.is_empty() {
        return;
    }
    xml.push_str("  <Sources>\n");
    for source in sources {
        let element = if source.kind == "audio" {
            "AudioSource"
        } else {
            "VideoSource"
        };
        let selection = if source.kind == "audio" {
            "AudioSelection"
        } else {
            "VideoSelection"
        };
        let empty = Vec::new();
        let codings = codings_by_source.get(&source.id).unwrap_or(&empty);
        let _ = write!(
            xml,
            "    <{element} guid=\"{guid}\" name=\"{name}\"",
            guid = source.id,
            name = escape_attr(&source.title),
        );
        if let Some(uri) = &source.uri {
            let _ = write!(xml, " path=\"{path}\"", path = escape_attr(uri));
        }
        if codings.is_empty() {
            xml.push_str("/>\n");
            continue;
        }
        xml.push_str(">\n");
        for coding in codings.iter() {
            let _ = writeln!(
                xml,
                "      <{selection} guid=\"{guid}\" begin=\"{begin}\" end=\"{end}\" \
                 creatingUser=\"{user}\">",
                guid = coding.id,
                begin = coding.start_ms,
                end = coding.end_ms,
                user = author_guid(&coding.author),
            );
            let _ = writeln!(
                xml,
                "        <Coding guid=\"{coding_guid}\" creatingUser=\"{user}\">",
                coding_guid = stable_id("refi-coding", &coding.id.to_string()),
                user = author_guid(&coding.author),
            );
            let _ = writeln!(
                xml,
                "          <CodeRef targetGUID=\"{code_guid}\"/>",
                code_guid = coding.code_id,
            );
            xml.push_str("        </Coding>\n");
            let _ = writeln!(xml, "      </{selection}>");
        }
        let _ = writeln!(xml, "    </{element}>");
    }
    xml.push_str("  </Sources>\n");
}

fn author_guid(author: &str) -> Uuid {
    stable_id("refi-user", author)
}

/// Escape an XML attribute value. `Code.name`, `Source.title`, and author
/// names are researcher-authored free text and may contain any of the five
/// characters XML attribute values must escape.
fn escape_attr(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CodingOrigin, NewCoding, NewSource, SourceKind};

    #[test]
    fn export_is_well_formed_xml_with_codebook_hierarchy() {
        let mut store = ResearchStore::open(":memory:").unwrap();
        let project = store.create_project("Study", "").unwrap();
        let source = NewSource {
            id: Uuid::new_v4(),
            project_id: project.id,
            recording_id: None,
            kind: SourceKind::Video,
            title: "Episode <1> & \"finale\"".into(),
            uri: Some("file:///episode.mkv".into()),
            duration_ms: Some(60_000),
            attributes: serde_json::json!({}),
        };
        store.upsert_source(&source).unwrap();
        let parent = Code {
            id: Uuid::new_v4(),
            project_id: project.id,
            parent_id: None,
            name: "Community".into(),
            description: String::new(),
            color: "#7c5cff".into(),
        };
        store.create_code(&parent).unwrap();
        let child = Code {
            id: Uuid::new_v4(),
            project_id: project.id,
            parent_id: Some(parent.id),
            name: "Belonging".into(),
            description: String::new(),
            color: "#00e5ff".into(),
        };
        store.create_code(&child).unwrap();
        store
            .add_coding(&NewCoding {
                id: Uuid::new_v4(),
                project_id: project.id,
                source_id: source.id,
                code_id: child.id,
                start_ms: 1_000,
                end_ms: 2_500,
                excerpt: "we built this together".into(),
                note: String::new(),
                author: "analyst@example.test".into(),
                origin: CodingOrigin::Human,
                confidence: None,
                provenance_id: None,
            })
            .unwrap();

        let xml = store.export_refi(project.id).unwrap();

        // Well-formedness: a minimal recursive-descent parity check for tag
        // nesting, since this crate has no XML parser dependency. Every
        // opening tag must have a matching close, in order.
        assert_well_formed(&xml);

        assert!(xml.contains("xmlns=\"urn:QDA-XML:project:1.0\""));
        assert!(xml.contains("<CodeBook>"));
        assert!(xml.contains(&format!("name=\"{}\"", "Community")));
        assert!(xml.contains(&format!("name=\"{}\"", "Belonging")));
        // Hierarchy: Belonging's <Code> must appear nested inside Community's.
        let community_start = xml.find("name=\"Community\"").unwrap();
        let belonging_pos = xml.find("name=\"Belonging\"").unwrap();
        let community_close = xml[community_start..].find("</Code>").unwrap() + community_start;
        assert!(belonging_pos > community_start && belonging_pos < community_close);
        // Escaped title survives.
        assert!(xml.contains("Episode &lt;1&gt; &amp; &quot;finale&quot;"));
        assert!(xml.contains("<VideoSource"));
        assert!(xml.contains("<VideoSelection"));
        assert!(xml.contains("begin=\"1000\""));
        assert!(xml.contains("end=\"2500\""));
    }

    #[test]
    fn audio_sources_export_as_audio_selection() {
        let mut store = ResearchStore::open(":memory:").unwrap();
        let project = store.create_project("Study", "").unwrap();
        let source = NewSource {
            id: Uuid::new_v4(),
            project_id: project.id,
            recording_id: None,
            kind: SourceKind::Audio,
            title: "Interview".into(),
            uri: None,
            duration_ms: None,
            attributes: serde_json::json!({}),
        };
        store.upsert_source(&source).unwrap();
        let code = Code {
            id: Uuid::new_v4(),
            project_id: project.id,
            parent_id: None,
            name: "Theme".into(),
            description: String::new(),
            color: "#ffffff".into(),
        };
        store.create_code(&code).unwrap();
        store
            .add_coding(&NewCoding {
                id: Uuid::new_v4(),
                project_id: project.id,
                source_id: source.id,
                code_id: code.id,
                start_ms: 0,
                end_ms: 100,
                excerpt: String::new(),
                note: String::new(),
                author: "a".into(),
                origin: CodingOrigin::Human,
                confidence: None,
                provenance_id: None,
            })
            .unwrap();
        let xml = store.export_refi(project.id).unwrap();
        assert_well_formed(&xml);
        assert!(xml.contains("<AudioSource"));
        assert!(xml.contains("<AudioSelection"));
    }

    #[test]
    fn model_and_import_origin_codings_are_excluded() {
        let mut store = ResearchStore::open(":memory:").unwrap();
        let project = store.create_project("Study", "").unwrap();
        let source = NewSource {
            id: Uuid::new_v4(),
            project_id: project.id,
            recording_id: None,
            kind: SourceKind::Video,
            title: "Episode".into(),
            uri: None,
            duration_ms: None,
            attributes: serde_json::json!({}),
        };
        store.upsert_source(&source).unwrap();
        let code = Code {
            id: Uuid::new_v4(),
            project_id: project.id,
            parent_id: None,
            name: "Auto".into(),
            description: String::new(),
            color: "#ffffff".into(),
        };
        store.create_code(&code).unwrap();
        store
            .add_coding(&NewCoding {
                id: Uuid::new_v4(),
                project_id: project.id,
                source_id: source.id,
                code_id: code.id,
                start_ms: 0,
                end_ms: 100,
                excerpt: String::new(),
                note: String::new(),
                author: "model".into(),
                origin: CodingOrigin::Model,
                confidence: Some(0.9),
                provenance_id: None,
            })
            .unwrap();
        let xml = store.export_refi(project.id).unwrap();
        assert_well_formed(&xml);
        assert!(!xml.contains("<VideoSelection"));
        assert!(!xml.contains("<Users>"));
    }

    /// Minimal well-formedness check: every `<Tag ...>` has a matching
    /// `</Tag>` (or is self-closing `.../>`), correctly nested, using a
    /// stack — enough to catch mismatched or unclosed elements without
    /// pulling in an XML parser dependency.
    fn assert_well_formed(xml: &str) {
        let mut stack: Vec<String> = Vec::new();
        let mut rest = xml;
        while let Some(lt) = rest.find('<') {
            let gt = rest[lt..].find('>').expect("unterminated tag") + lt;
            let tag = &rest[lt + 1..gt];
            if let Some(stripped) = tag.strip_prefix('?') {
                let _ = stripped;
            } else if let Some(name) = tag.strip_prefix('/') {
                let expected = stack
                    .pop()
                    .unwrap_or_else(|| panic!("closing tag </{name}> with no matching open tag"));
                let name = name.split_whitespace().next().unwrap_or(name);
                assert_eq!(expected, name, "mismatched closing tag");
            } else if !tag.ends_with('/') {
                let name = tag.split_whitespace().next().unwrap_or(tag).to_string();
                stack.push(name);
            }
            rest = &rest[gt + 1..];
        }
        assert!(
            stack.is_empty(),
            "unclosed tags remain: {stack:?} in document:\n{xml}"
        );
    }
}
