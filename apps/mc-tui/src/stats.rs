use std::collections::BTreeMap;

use mc_protocol::events::StatDataModel;

// ── Constants ─────────────────────────────────────────────────────────────

/// Maximum number of (tick, value) points kept per series.
pub const MAX_STAT_POINTS: usize = 300;

/// Ordered category keys for the stats tab.
pub const STAT_CATEGORY_ORDER: &[&str] = &[
    "server-performance",
    "memory",
    "scripting",
    "client",
    "uncategorized",
];

// ── StatSeries ────────────────────────────────────────────────────────────

/// A single bucketed time-series.
#[derive(Debug, Clone)]
pub struct StatSeries {
    /// Dot-separated flattened path of the stat (e.g. `"server_tick_timings.tick"`).
    pub name: String,
    /// Same as `name`; kept for symmetry with the desktop build.
    pub path: String,
    /// Tick markers aligned one-to-one with `values`.
    pub ticks: Vec<u64>,
    /// Numeric values aligned one-to-one with `ticks`.
    pub values: Vec<f64>,
}

// ── Group / Category metadata structures ──────────────────────────────────

/// A group of series that share the same top-level path component.
#[derive(Debug, Clone)]
pub struct StatGroup {
    /// Top-level name (first path component, e.g. `"server_tick_timings"`).
    pub name: String,
    /// Series in this group, sorted by path.
    pub series: Vec<StatSeries>,
}

/// A category with display metadata, containing zero or more groups.
#[derive(Debug, Clone)]
pub struct StatCategory {
    pub key: &'static str,
    pub label: &'static str,
    pub icon: &'static str,
    pub groups: Vec<StatGroup>,
}

// ── Category map helpers ──────────────────────────────────────────────────

/// Return the category key for a given top-level group name.
fn stat_group_category(group_name: &str) -> &'static str {
    match group_name {
        "server_tick_timings" => "server-performance",
        "entities" => "server-performance",
        "chunks" => "server-performance",
        "networking" => "server-performance",
        "app_memory" => "memory",
        "dynamic_property_values" => "memory",
        "handle_counts" => "scripting",
        "fine_grained_subscribers" => "scripting",
        "client_stats" => "client",
        _ => "uncategorized",
    }
}

/// Return the display metadata for a category key.
///
/// Icons are terminal-safe single-cell glyphs, not desktop icon names.
fn stat_category_meta(key: &str) -> (&'static str, &'static str) {
    match key {
        "server-performance" => ("Server Performance", "◈"),
        "memory" => ("Memory", "◆"),
        "scripting" => ("Scripting", "◇"),
        "client" => ("Client", "◐"),
        "uncategorized" => ("Uncategorized", "○"),
        _ => ("Uncategorized", "○"),
    }
}

// ── Core accumulation ─────────────────────────────────────────────────────

/// Extract the last numeric value from a JSON array.
///
/// Returns `None` when the array is empty, the last element is not a number,
/// or the last element is a string that cannot be parsed as a float.
pub fn extract_last_number(values: &[serde_json::Value]) -> Option<f64> {
    let last = values.last()?;
    match last {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.parse::<f64>().ok(),
        _ => None,
    }
}

/// Recursively flatten `StatDataModel` entries into dot-separated paths and
/// accumulate numeric values into `collection`.
///
/// * `prefix` – accumulated dot-path from parent levels (empty at root).
pub fn accumulate_stats(
    collection: &mut BTreeMap<String, StatSeries>,
    stats: &[StatDataModel],
    tick: u64,
    prefix: &str,
) {
    for stat in stats {
        let path = if prefix.is_empty() {
            stat.name.clone()
        } else {
            format!("{}.{}", prefix, stat.name)
        };

        if let Some(value) = extract_last_number(&stat.values) {
            let entry = collection
                .entry(path.clone())
                .or_insert_with(|| StatSeries {
                    name: path.clone(),
                    path: path.clone(),
                    ticks: Vec::new(),
                    values: Vec::new(),
                });
            entry.ticks.push(tick);
            entry.values.push(value);
            if entry.ticks.len() > MAX_STAT_POINTS {
                entry.ticks.remove(0);
                entry.values.remove(0);
            }
        }

        // Recurse into children.
        if !stat.children.is_empty() {
            accumulate_stats(collection, &stat.children, tick, &path);
        }
    }
}

// ── Formatting helpers ────────────────────────────────────────────────────

/// Return a short display name by stripping the first path component.
pub fn short_name(path: &str) -> String {
    let parts: Vec<&str> = path.split('.').collect();
    if parts.len() > 1 {
        parts[1..].join(".")
    } else {
        parts[0].to_string()
    }
}

/// Extract the client ID from a `client_stats.<id>.*` path.
pub fn get_client_id(path: &str) -> Option<String> {
    let parts: Vec<&str> = path.split('.').collect();
    if parts.len() >= 3 && parts[0] == "client_stats" {
        Some(parts[1].to_string())
    } else {
        None
    }
}

/// Parse a fine-grained subscriber path. The addon is exactly one component;
/// everything after it is the event name.
pub fn get_subscriber_parts(path: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = path.split('.').collect();
    if parts.len() >= 3
        && parts[0] == "fine_grained_subscribers"
        && parts[1..].iter().all(|part| !part.is_empty())
    {
        Some((parts[1].to_string(), parts[2..].join(".")))
    } else {
        None
    }
}

/// Whether the group name relates to memory (used for MB scaling).
pub fn is_memory_group(group_name: &str) -> bool {
    group_name.to_lowercase().contains("memory")
}

/// Format a raw stat value into a human-readable string.
pub fn format_stat_value(val: f64) -> String {
    let abs = val.abs();
    if abs >= 1_000_000_000.0 {
        format!("{:.2}B", val / 1_000_000_000.0)
    } else if abs >= 1_000_000.0 {
        format!("{:.2}M", val / 1_000_000.0)
    } else if abs >= 1_000.0 {
        format!("{:.1}K", val / 1_000.0)
    } else {
        format!("{:.1}", val)
    }
}

/// Format a memory value as MB.
pub fn format_memory_value(mb: f64) -> String {
    format!("{:.2} MB", mb)
}

/// Format a group value, scaling to MB for memory groups.
pub fn format_group_value(group_name: &str, val: Option<f64>) -> String {
    match val {
        None => "—".to_string(),
        Some(v) => {
            if is_memory_group(group_name) {
                format_memory_value(v / 1_048_576.0)
            } else {
                format_stat_value(v)
            }
        }
    }
}

/// Scale series values for display: divide by 1_048_576 for memory groups.
pub fn scale_series_for_display(series: &StatSeries, group_name: &str) -> StatSeries {
    if !is_memory_group(group_name) {
        return series.clone();
    }
    StatSeries {
        name: series.name.clone(),
        path: series.path.clone(),
        ticks: series.ticks.clone(),
        values: series.values.iter().map(|v| v / 1_048_576.0).collect(),
    }
}

// ── Build read-oriented structures ────────────────────────────────────────

/// Group series by their top-level path component, sorted deterministically.
pub fn build_chart_groups(collection: &BTreeMap<String, StatSeries>) -> Vec<StatGroup> {
    let mut groups: BTreeMap<String, Vec<StatSeries>> = BTreeMap::new();
    for series in collection.values() {
        let top = series.path.split('.').next().unwrap_or("");
        groups
            .entry(top.to_string())
            .or_default()
            .push(series.clone());
    }
    groups
        .into_iter()
        .map(|(name, series)| StatGroup { name, series })
        .collect()
}

/// Assign groups to categories using the static category map.
pub fn build_categorized_groups(groups: &[StatGroup]) -> Vec<StatCategory> {
    let mut cat_map: BTreeMap<&str, Vec<StatGroup>> = BTreeMap::new();
    for group in groups {
        let cat_key = stat_group_category(&group.name);
        cat_map.entry(cat_key).or_default().push(group.clone());
    }
    STAT_CATEGORY_ORDER
        .iter()
        .filter_map(|&key| cat_map.remove(key).map(|groups| (key, groups)))
        .map(|(key, groups)| {
            let (label, icon) = stat_category_meta(key);
            StatCategory {
                key,
                label,
                icon,
                groups,
            }
        })
        .collect()
}

/// Collect all unique client IDs from `client_stats.*` paths, sorted.
pub fn build_client_ids(collection: &BTreeMap<String, StatSeries>) -> Vec<String> {
    let mut ids: Vec<String> = collection
        .keys()
        .filter_map(|path| get_client_id(path))
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// Collect unique, valid subscriber addon IDs in deterministic order.
pub fn build_subscriber_addon_ids(collection: &BTreeMap<String, StatSeries>) -> Vec<String> {
    let mut ids: Vec<String> = collection
        .keys()
        .filter_map(|path| get_subscriber_parts(path).map(|(addon, _)| addon))
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

// ── StatsState (ephemeral app-owned state) ────────────────────────────────

/// Ephemeral stats accumulation state.
///
/// Owned by [`App`](crate::app::App).  Use `clear()` to reset; do not persist.
#[derive(Debug, Clone)]
pub struct StatsState {
    collection: BTreeMap<String, StatSeries>,
}

impl StatsState {
    /// Create an empty stats state.
    pub fn new() -> Self {
        Self {
            collection: BTreeMap::new(),
        }
    }

    /// Clear all accumulated stats.
    pub fn clear(&mut self) {
        self.collection.clear();
    }

    /// Accumulate a batch of `StatDataModel` entries at the given tick.
    pub fn accumulate(&mut self, stats: &[StatDataModel], tick: u64) {
        accumulate_stats(&mut self.collection, stats, tick, "");
    }

    // ── Read APIs ───────────────────────────────────────────────────────

    /// Whether no stats have been accumulated.
    pub fn is_empty(&self) -> bool {
        self.collection.is_empty()
    }

    /// Number of series currently tracked.
    pub fn series_count(&self) -> usize {
        self.collection.len()
    }

    /// Reference to the raw collection (for iteration, serialization, etc.).
    pub fn collection(&self) -> &BTreeMap<String, StatSeries> {
        &self.collection
    }

    /// Build groups ordered by top-level sort.
    pub fn groups(&self) -> Vec<StatGroup> {
        build_chart_groups(&self.collection)
    }

    /// Build categorized groups for UI rendering.
    pub fn categories(&self) -> Vec<StatCategory> {
        let groups = self.groups();
        build_categorized_groups(&groups)
    }

    /// Collect sorted unique client IDs.
    pub fn client_ids(&self) -> Vec<String> {
        build_client_ids(&self.collection)
    }

    pub fn subscriber_addon_ids(&self) -> Vec<String> {
        build_subscriber_addon_ids(&self.collection)
    }
}

impl Default for StatsState {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_stat(
        name: &str,
        values: Vec<serde_json::Value>,
        children: Vec<StatDataModel>,
    ) -> StatDataModel {
        StatDataModel {
            name: name.into(),
            values,
            children,
            should_aggregate: false,
        }
    }

    // ── extract_last_number ─────────────────────────────────────────────

    #[test]
    fn extract_last_number_from_numeric() {
        assert_eq!(extract_last_number(&[json!(42.5)]), Some(42.5));
        assert_eq!(
            extract_last_number(&[json!(1), json!(2), json!(3)]),
            Some(3.0)
        );
    }

    #[test]
    fn extract_last_number_from_string() {
        let v = extract_last_number(&[json!("42.5")]);
        assert_eq!(v, Some(42.5));
    }

    #[test]
    fn extract_last_number_returns_none_for_empty() {
        assert_eq!(extract_last_number(&[]), None);
    }

    #[test]
    fn extract_last_number_returns_none_for_non_numeric() {
        assert_eq!(extract_last_number(&[json!("abc")]), None);
        assert_eq!(extract_last_number(&[json!(null)]), None);
        assert_eq!(extract_last_number(&[json!([1, 2, 3])]), None);
    }

    // ── Flat accumulation ───────────────────────────────────────────────

    #[test]
    fn accumulate_flat_stats() {
        let mut col = BTreeMap::new();
        let stats = vec![make_stat("cpu", vec![json!(50.0)], vec![])];
        accumulate_stats(&mut col, &stats, 1, "");
        assert_eq!(col.len(), 1);
        let s = &col["cpu"];
        assert_eq!(s.ticks, vec![1]);
        assert_eq!(s.values, vec![50.0]);
    }

    // ── Recursive accumulation ──────────────────────────────────────────

    #[test]
    fn accumulate_recursive_stats() {
        let mut col = BTreeMap::new();
        let stats = vec![make_stat(
            "server_tick_timings",
            vec![],
            vec![
                make_stat("tick", vec![json!(10.0)], vec![]),
                make_stat("entity", vec![json!(20.0)], vec![]),
            ],
        )];
        accumulate_stats(&mut col, &stats, 5, "");
        assert_eq!(col.len(), 2);
        assert_eq!(col["server_tick_timings.tick"].ticks, vec![5]);
        assert_eq!(col["server_tick_timings.tick"].values, vec![10.0]);
        assert_eq!(col["server_tick_timings.entity"].values, vec![20.0]);
    }

    // ── Repeated ticks / path update ────────────────────────────────────

    #[test]
    fn repeated_ticks_append_to_same_path() {
        let mut col = BTreeMap::new();
        let stats = vec![make_stat("cpu", vec![json!(10.0)], vec![])];
        accumulate_stats(&mut col, &stats, 1, "");
        accumulate_stats(&mut col, &stats, 2, "");
        accumulate_stats(&mut col, &stats, 3, "");
        let s = &col["cpu"];
        assert_eq!(s.ticks, vec![1, 2, 3]);
        assert_eq!(s.values, vec![10.0, 10.0, 10.0]);
    }

    // ── Bounds and oldest removal (>300) ────────────────────────────────

    #[test]
    fn bounds_drops_oldest_when_exceeding_max() {
        let mut col = BTreeMap::new();
        let stats = vec![make_stat("x", vec![json!(1.0)], vec![])];
        for tick in 1..=MAX_STAT_POINTS + 10 {
            accumulate_stats(&mut col, &stats, tick as u64, "");
        }
        let s = &col["x"];
        assert_eq!(s.ticks.len(), MAX_STAT_POINTS);
        // The oldest remaining tick should be tick 11 (1..310, remove 1..10)
        assert_eq!(s.ticks[0], 11);
        assert_eq!(s.ticks[MAX_STAT_POINTS - 1], (MAX_STAT_POINTS + 10) as u64);
    }

    #[test]
    fn bounds_retains_exact_max() {
        let mut col = BTreeMap::new();
        let stats = vec![make_stat("y", vec![json!(1.0)], vec![])];
        for tick in 1..=MAX_STAT_POINTS {
            accumulate_stats(&mut col, &stats, tick as u64, "");
        }
        assert_eq!(col["y"].ticks.len(), MAX_STAT_POINTS);
        assert_eq!(col["y"].ticks[0], 1);
        assert_eq!(col["y"].ticks[MAX_STAT_POINTS - 1], MAX_STAT_POINTS as u64);
    }

    // ── Non-numeric skip ────────────────────────────────────────────────

    #[test]
    fn non_numeric_skipped() {
        let mut col = BTreeMap::new();
        let stats = vec![
            make_stat("a", vec![json!("not-a-number")], vec![]),
            make_stat("b", vec![json!(42.0)], vec![]),
        ];
        accumulate_stats(&mut col, &stats, 1, "");
        assert!(!col.contains_key("a"));
        assert!(col.contains_key("b"));
    }

    #[test]
    fn empty_values_skipped() {
        let mut col = BTreeMap::new();
        let stats = vec![make_stat("empty", vec![], vec![])];
        accumulate_stats(&mut col, &stats, 1, "");
        assert!(col.is_empty());
    }

    #[test]
    fn null_value_skipped() {
        let mut col = BTreeMap::new();
        let stats = vec![make_stat("null_val", vec![json!(null)], vec![])];
        accumulate_stats(&mut col, &stats, 1, "");
        assert!(col.is_empty());
    }

    // ── Deterministic ordering (BTreeMap) ───────────────────────────────

    #[test]
    fn deterministic_ordering() {
        let mut col = BTreeMap::new();
        let stats = vec![
            make_stat("z_last", vec![json!(3.0)], vec![]),
            make_stat("a_first", vec![json!(1.0)], vec![]),
            make_stat("m_mid", vec![json!(2.0)], vec![]),
        ];
        accumulate_stats(&mut col, &stats, 1, "");
        let keys: Vec<&str> = col.keys().map(|s| s.as_str()).collect();
        assert_eq!(keys, vec!["a_first", "m_mid", "z_last"]);
    }

    // ── short_name ──────────────────────────────────────────────────────

    #[test]
    fn short_name_strips_first_component() {
        assert_eq!(short_name("a.b.c"), "b.c");
        assert_eq!(short_name("single"), "single");
        assert_eq!(short_name("client_stats.abc.mem"), "abc.mem");
    }

    // ── get_client_id ───────────────────────────────────────────────────

    #[test]
    fn client_id_extracted_correctly() {
        assert_eq!(
            get_client_id("client_stats.uuid-123.memory"),
            Some("uuid-123".into())
        );
        assert_eq!(get_client_id("server_tick_timings.tick"), None);
        assert_eq!(get_client_id("client_stats"), None);
        assert_eq!(get_client_id(""), None);
    }

    #[test]
    fn subscriber_paths_are_strict_and_events_keep_remaining_components() {
        assert_eq!(
            get_subscriber_parts("fine_grained_subscribers.alpha.block.place"),
            Some(("alpha".into(), "block.place".into()))
        );
        assert!(get_subscriber_parts("fine_grained_subscribers.alpha").is_none());
        assert!(get_subscriber_parts("fine_grained_subscribers..place").is_none());
        assert!(get_subscriber_parts("other.alpha.place").is_none());
    }

    // ── is_memory_group ─────────────────────────────────────────────────

    #[test]
    fn memory_group_detection() {
        assert!(is_memory_group("app_memory"));
        assert!(is_memory_group("memory"));
        assert!(is_memory_group("MEMORY"));
        assert!(!is_memory_group("server_tick_timings"));
    }

    // ── format helpers ──────────────────────────────────────────────────

    #[test]
    fn format_stat_value_scales() {
        assert_eq!(format_stat_value(123.4), "123.4");
        assert_eq!(format_stat_value(1_500.0), "1.5K");
        assert_eq!(format_stat_value(2_000_000.0), "2.00M");
        assert_eq!(format_stat_value(1_500_000_000.0), "1.50B");
    }

    #[test]
    fn format_memory_value_shows_mb() {
        assert_eq!(format_memory_value(256.0), "256.00 MB");
        assert_eq!(format_memory_value(0.5), "0.50 MB");
    }

    #[test]
    fn format_group_value_scales_memory() {
        // 1048576 bytes = 1 MB
        let val = format_group_value("app_memory", Some(1_048_576.0));
        assert_eq!(val, "1.00 MB");
        // Non-memory group uses regular formatting
        let val = format_group_value("server_tick_timings", Some(1500.0));
        assert_eq!(val, "1.5K");
    }

    #[test]
    fn format_group_value_none_returns_dash() {
        assert_eq!(format_group_value("anything", None), "—");
    }

    // ── scale_series_for_display ────────────────────────────────────────

    #[test]
    fn scale_series_memory_converts_to_mb() {
        let series = StatSeries {
            name: "app_memory.used".into(),
            path: "app_memory.used".into(),
            ticks: vec![1, 2],
            values: vec![1_048_576.0, 2_097_152.0],
        };
        let scaled = scale_series_for_display(&series, "app_memory");
        assert!((scaled.values[0] - 1.0).abs() < 1e-6);
        assert!((scaled.values[1] - 2.0).abs() < 1e-6);
    }

    #[test]
    fn scale_series_non_memory_passthrough() {
        let series = StatSeries {
            name: "tick.cpu".into(),
            path: "tick.cpu".into(),
            ticks: vec![1],
            values: vec![50.0],
        };
        let scaled = scale_series_for_display(&series, "server_tick_timings");
        assert_eq!(scaled.values, vec![50.0]);
    }

    // ── build_chart_groups ──────────────────────────────────────────────

    #[test]
    fn chart_groups_by_top_level() {
        let mut col = BTreeMap::new();
        col.insert(
            "server_tick_timings.tick".into(),
            StatSeries {
                name: "server_tick_timings.tick".into(),
                path: "server_tick_timings.tick".into(),
                ticks: vec![1],
                values: vec![10.0],
            },
        );
        col.insert(
            "server_tick_timings.entity".into(),
            StatSeries {
                name: "server_tick_timings.entity".into(),
                path: "server_tick_timings.entity".into(),
                ticks: vec![1],
                values: vec![20.0],
            },
        );
        col.insert(
            "app_memory.used".into(),
            StatSeries {
                name: "app_memory.used".into(),
                path: "app_memory.used".into(),
                ticks: vec![1],
                values: vec![30.0],
            },
        );
        let groups = build_chart_groups(&col);
        assert_eq!(groups.len(), 2);
        // BTreeMap iteration is deterministic — "app_memory" < "server_tick_timings"
        assert_eq!(groups[0].name, "app_memory");
        assert_eq!(groups[0].series.len(), 1);
        assert_eq!(groups[1].name, "server_tick_timings");
        assert_eq!(groups[1].series.len(), 2);
    }

    // ── build_categorized_groups ────────────────────────────────────────

    #[test]
    fn categorized_groups_assigns_correctly() {
        let groups = vec![
            StatGroup {
                name: "server_tick_timings".into(),
                series: vec![],
            },
            StatGroup {
                name: "app_memory".into(),
                series: vec![],
            },
            StatGroup {
                name: "handle_counts".into(),
                series: vec![],
            },
        ];
        let cats = build_categorized_groups(&groups);
        assert_eq!(cats.len(), 3);
        assert_eq!(cats[0].key, "server-performance");
        assert_eq!(cats[0].groups[0].name, "server_tick_timings");
        assert_eq!(cats[1].key, "memory");
        assert_eq!(cats[1].groups[0].name, "app_memory");
        assert_eq!(cats[2].key, "scripting");
        assert_eq!(cats[2].groups[0].name, "handle_counts");
    }

    #[test]
    fn categorized_groups_assigns_dynamic_property_to_memory() {
        let groups = vec![StatGroup {
            name: "dynamic_property_values".into(),
            series: vec![],
        }];
        let cats = build_categorized_groups(&groups);
        assert_eq!(cats.len(), 1);
        assert_eq!(cats[0].key, "memory");
    }

    #[test]
    fn categorized_groups_assigns_client_stats() {
        let groups = vec![StatGroup {
            name: "client_stats".into(),
            series: vec![],
        }];
        let cats = build_categorized_groups(&groups);
        assert_eq!(cats.len(), 1);
        assert_eq!(cats[0].key, "client");
    }

    #[test]
    fn categorized_groups_ignores_empty_categories() {
        let cats = build_categorized_groups(&[]);
        assert_eq!(cats.len(), 0);
    }

    // ── build_client_ids ────────────────────────────────────────────────

    #[test]
    fn client_ids_collected_and_sorted() {
        let mut col = BTreeMap::new();
        for (path, val) in [
            ("client_stats.b.mem", 1.0),
            ("client_stats.a.cpu", 2.0),
            ("server_tick_timings.tick", 3.0),
        ] {
            col.insert(
                path.into(),
                StatSeries {
                    name: path.into(),
                    path: path.into(),
                    ticks: vec![1],
                    values: vec![val],
                },
            );
        }
        let ids = build_client_ids(&col);
        assert_eq!(ids, vec!["a", "b"]);
    }

    #[test]
    fn subscriber_addon_ids_are_sorted_and_ignore_other_groups() {
        let mut col = BTreeMap::new();
        for path in [
            "fine_grained_subscribers.zeta.event",
            "fine_grained_subscribers.alpha.event",
            "handle_counts.alpha.event",
            "fine_grained_subscribers.zeta.other",
        ] {
            col.insert(
                path.into(),
                StatSeries {
                    name: path.into(),
                    path: path.into(),
                    ticks: vec![],
                    values: vec![],
                },
            );
        }
        assert_eq!(build_subscriber_addon_ids(&col), vec!["alpha", "zeta"]);
    }

    // ── StatsState ──────────────────────────────────────────────────────

    #[test]
    fn stats_state_new_is_empty() {
        let state = StatsState::new();
        assert!(state.is_empty());
        assert_eq!(state.series_count(), 0);
    }

    #[test]
    fn stats_state_accumulate() {
        let mut state = StatsState::new();
        let stats = vec![make_stat("cpu", vec![json!(50.0)], vec![])];
        state.accumulate(&stats, 1);
        assert!(!state.is_empty());
        assert_eq!(state.series_count(), 1);
    }

    #[test]
    fn stats_state_clear_resets() {
        let mut state = StatsState::new();
        let stats = vec![make_stat("cpu", vec![json!(50.0)], vec![])];
        state.accumulate(&stats, 1);
        assert!(!state.is_empty());
        state.clear();
        assert!(state.is_empty());
        assert_eq!(state.series_count(), 0);
    }

    #[test]
    fn stats_state_groups_and_categories() {
        let mut state = StatsState::new();
        let stats = vec![make_stat(
            "server_tick_timings",
            vec![],
            vec![make_stat("tick", vec![json!(10.0)], vec![])],
        )];
        state.accumulate(&stats, 1);
        assert_eq!(state.groups().len(), 1);
        assert_eq!(state.categories().len(), 1);
        assert_eq!(state.categories()[0].key, "server-performance");
    }

    #[test]
    fn stats_state_client_ids() {
        let mut state = StatsState::new();
        let stats = vec![make_stat(
            "client_stats",
            vec![],
            vec![make_stat(
                "uuid-abc",
                vec![],
                vec![make_stat("mem", vec![json!(100.0)], vec![])],
            )],
        )];
        state.accumulate(&stats, 1);
        let ids = state.client_ids();
        assert_eq!(ids, vec!["uuid-abc"]);
    }
}
