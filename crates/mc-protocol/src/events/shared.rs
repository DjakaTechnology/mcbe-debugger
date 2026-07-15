use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginDetails {
    pub name: String,
    pub module_uuid: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize_repr, Deserialize_repr)]
#[repr(u8)]
pub enum LogLevel {
    Log = 0,
    Warn = 1,
    Error = 2,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatDataModel {
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<StatDataModel>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<serde_json::Value>,
    #[serde(default)]
    pub should_aggregate: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticsDataSource {
    Server,
    Client,
    ServerScript,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticsDisplayType {
    LineChart,
    StackedLineChart,
    StackedBarChart,
    Table,
    MultiColumnTable,
    DynamicPropertiesTable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticsTabDescriptor {
    pub name: String,
    pub stat_group_id: String,
    pub data_source: DiagnosticsDataSource,
    pub display_type: DiagnosticsDisplayType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tick_range: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_scalar: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_value: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_labels: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub statistic_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub statistic_ids: Option<Vec<String>>,
}
