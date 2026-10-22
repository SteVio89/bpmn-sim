//! Minimal DMN reading: which decisions a file defines (evaluation comes in a later milestone).

use std::collections::BTreeMap;

use crate::error::ParseError;

#[derive(Debug, Clone, Default)]
pub struct DmnFile {
    pub name: String,
    /// Decision id -> decision name.
    pub decisions: BTreeMap<String, Option<String>>,
}

pub fn parse_dmn(file: &str, xml: &str) -> Result<DmnFile, ParseError> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| ParseError::Xml {
        file: file.to_string(),
        message: e.to_string(),
    })?;
    let root = doc.root_element();
    let is_dmn = root.tag_name().name() == "definitions"
        && root
            .tag_name()
            .namespace()
            .is_some_and(|n| n.contains("/DMN/"));
    if !is_dmn {
        return Err(ParseError::NotDmn {
            file: file.to_string(),
        });
    }
    let mut out = DmnFile {
        name: file.to_string(),
        ..DmnFile::default()
    };
    for d in root
        .children()
        .filter(|c| c.is_element() && c.tag_name().name() == "decision")
    {
        let Some(id) = d.attribute("id") else {
            return Err(ParseError::Element {
                file: file.to_string(),
                element: "<decision>".into(),
                message: "element has no 'id' attribute".into(),
            });
        };
        out.decisions
            .insert(id.to_string(), d.attribute("name").map(str::to_string));
    }
    Ok(out)
}
