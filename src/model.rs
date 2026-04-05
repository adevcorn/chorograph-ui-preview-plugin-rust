/// Shared data model — mirrors UIPreviewPayload.swift on the host side.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum UIPreviewCategory {
    Container,
    Interactive,
    Display,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UIPreviewNode {
    pub r#type: String,
    pub category: UIPreviewCategory,
    pub label: Option<String>,
    pub children: Vec<UIPreviewNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UIPreviewPayload {
    pub file_path: String,
    pub framework: String,
    pub component_name: String,
    pub root: UIPreviewNode,
}
