//! Request and response types shared by every ADT backend.

use serde::{Deserialize, Serialize};

pub const MAX_TABLE_ROWS: usize = 1000;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AbapObjectKind {
    Program,
    Class,
    Interface,
    Include,
    FunctionGroup,
    FunctionModule,
    Table,
    Structure,
    DataElement,
    Domain,
    Package,
    CdsView,
    BehaviorDefinition,
    ServiceDefinition,
    MetadataExtension,
    EnhancementSpot,
    Transaction,
}

impl AbapObjectKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Program => "Program",
            Self::Class => "Class",
            Self::Interface => "Interface",
            Self::Include => "Include",
            Self::FunctionGroup => "Function Group",
            Self::FunctionModule => "Function Module",
            Self::Table => "Table",
            Self::Structure => "Structure",
            Self::DataElement => "Data Element",
            Self::Domain => "Domain",
            Self::Package => "Package",
            Self::CdsView => "CDS View",
            Self::BehaviorDefinition => "Behavior Definition",
            Self::ServiceDefinition => "Service Definition",
            Self::MetadataExtension => "Metadata Extension",
            Self::EnhancementSpot => "Enhancement Spot",
            Self::Transaction => "Transaction",
        }
    }

    /// ADT URI fragment, e.g. `programs/programs/Z_FOO/source/main`.
    pub fn adt_path(self, name: &str) -> String {
        let n = name.to_lowercase();
        match self {
            Self::Program => format!("/sap/bc/adt/programs/programs/{n}/source/main"),
            Self::Class => format!("/sap/bc/adt/oo/classes/{n}/source/main"),
            Self::Interface => format!("/sap/bc/adt/oo/interfaces/{n}/source/main"),
            Self::Include => format!("/sap/bc/adt/programs/includes/{n}/source/main"),
            Self::FunctionGroup => format!("/sap/bc/adt/functions/groups/{n}/source/main"),
            Self::FunctionModule => format!("/sap/bc/adt/functions/groups/{{group}}/fmodules/{n}/source/main"),
            Self::Table => format!("/sap/bc/adt/ddic/tables/{n}/source/main"),
            Self::CdsView => format!("/sap/bc/adt/ddic/ddl/sources/{n}/source/main"),
            Self::Package => format!("/sap/bc/adt/repository/nodestructure?parent_name={n}&parent_type=DEVC%2FK"),
            _ => format!("/sap/bc/adt/repository/informationsystem/objectproperties/values?uri=/sap/bc/adt/{}", name),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgramSource {
    pub name: String,
    pub kind: AbapObjectKind,
    /// Package / development class.
    pub package: Option<String>,
    /// Description / short text from object header.
    pub description: Option<String>,
    pub source: String,
    /// Whether the object is currently activated (vs. saved-but-inactive).
    pub active: bool,
    /// Lines counted from the source.
    pub line_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CdsView {
    pub name: String,
    pub source: String,
    /// e.g. `Z_DEMO_CDS`
    pub root_entity: String,
    /// CDS annotations distilled into a structured map for quick access.
    pub annotations: serde_json::Value,
    pub line_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageMember {
    pub name: String,
    pub kind: AbapObjectKind,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageContents {
    pub package: String,
    pub description: Option<String>,
    pub members: Vec<PackageMember>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdtSearchRequest {
    pub query: String,
    #[serde(default)]
    pub kind: Option<AbapObjectKind>,
    #[serde(default = "default_max_results")]
    pub max_results: usize,
}

fn default_max_results() -> usize { 25 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdtSearchHit {
    pub name: String,
    pub kind: AbapObjectKind,
    pub description: Option<String>,
    pub package: Option<String>,
    pub score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhereUsedRequest {
    pub name: String,
    pub kind: AbapObjectKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhereUsedHit {
    pub object: String,
    pub kind: AbapObjectKind,
    /// Where in the object the reference appears (e.g. line, method).
    pub location: String,
    /// e.g. `read`, `write`, `call`, `inherits`, `implements`.
    pub usage: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableRow {
    pub values: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivationRequest {
    pub name: String,
    pub kind: AbapObjectKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivationOutcome {
    pub name: String,
    pub kind: AbapObjectKind,
    pub activated: bool,
    pub messages: Vec<String>,
}
