use std::collections::BTreeMap;

use anyhow::{Result, anyhow};
use claude_mcp::{
    McpClientInfo, McpListChangedSurface, McpServerConfig, McpServerInspection, inspect_server,
    normalization::{build_mcp_prompt_command_name, build_mcp_tool_name, normalize_name_for_mcp},
};
use claude_ui_bridge::UiRuntimeMcpServerStatus;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::{
    RuntimeMcpServerPolicyEntry, ToolRuntimeHints, ToolSpec, current_runtime_mcp_observation,
    current_tool_runtime_policy, mcp_runtime::RuntimeMcpServerObservation, tool_allowed_by_policy,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeMcpClientDescriptor {
    pub server_name: String,
    pub normalized_server_name: String,
    pub instructions: Option<String>,
    pub supports_resources: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeMcpToolDescriptor {
    pub tool_spec: ToolSpec,
    pub server_name: String,
    pub normalized_server_name: String,
    pub tool_name: String,
    pub normalized_tool_name: String,
    pub server_config: McpServerConfig,
    pub annotations: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeMcpPromptCommandDescriptor {
    pub command_name: String,
    pub server_name: String,
    pub normalized_server_name: String,
    pub prompt_name: String,
    pub description: String,
    pub arg_names: Vec<String>,
    pub server_config: McpServerConfig,
    pub prompt: claude_mcp::McpPromptDescriptor,
}

impl RuntimeMcpToolDescriptor {
    #[must_use]
    pub fn qualified_name(&self) -> &str {
        &self.tool_spec.name
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RuntimeMcpCatalog {
    pub clients: Vec<RuntimeMcpClientDescriptor>,
    pub tools: Vec<RuntimeMcpToolDescriptor>,
    pub prompts: Vec<RuntimeMcpPromptCommandDescriptor>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
struct CachedInspection {
    server_config: McpServerConfig,
    inspection: McpServerInspection,
}

static RUNTIME_MCP_INSPECTION_CACHE: Lazy<Mutex<BTreeMap<String, CachedInspection>>> =
    Lazy::new(|| Mutex::new(BTreeMap::new()));

fn inspection_cache_key(entry: &RuntimeMcpServerPolicyEntry) -> String {
    format!("{}::{}", entry.config_path.display(), entry.server.name)
}

fn observation_entry_matches_policy_entry(
    observation: &RuntimeMcpServerObservation,
    entry: &RuntimeMcpServerPolicyEntry,
) -> bool {
    observation.entry.origin_kind == entry.origin_kind
        && observation.entry.origin_name == entry.origin_name
        && observation.entry.config_path == entry.config_path
        && observation.entry.server == entry.server
}

fn snapshot_inspection_for_entry(
    entry: &RuntimeMcpServerPolicyEntry,
) -> Option<Result<McpServerInspection>> {
    let observation = current_runtime_mcp_observation()?;
    let server = observation
        .servers
        .iter()
        .find(|server| observation_entry_matches_policy_entry(server, entry))?;
    if let Some(inspection) = &server.inspection {
        return Some(Ok(inspection.clone()));
    }
    if server.status == UiRuntimeMcpServerStatus::Failed {
        let message = server
            .error
            .clone()
            .unwrap_or_else(|| "runtime MCP observation recorded a failed connection".to_owned());
        return Some(Err(anyhow!(message)));
    }
    None
}

fn annotation_value<'a>(annotations: &'a Value, key: &str) -> Option<&'a Value> {
    annotations
        .get(key)
        .or_else(|| annotations.get("_meta").and_then(|meta| meta.get(key)))
}

fn annotation_hint_is_true(annotations: &Value, key: &str) -> bool {
    annotation_value(annotations, key)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn annotation_hint_string(annotations: &Value, key: &str) -> Option<String> {
    annotation_value(annotations, key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn annotation_hint_strings(annotations: &Value, key: &str) -> Vec<String> {
    match annotation_value(annotations, key) {
        Some(Value::String(value)) => value
            .split(|ch: char| ch == ',' || ch == ';' || ch.is_whitespace())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

fn annotation_hint_bool(annotations: &Value, key: &str) -> Option<bool> {
    annotations.get(key).and_then(Value::as_bool).or_else(|| {
        annotations
            .get("_meta")
            .and_then(|meta| meta.get(key))
            .and_then(Value::as_bool)
    })
}

fn inspection_supports_resources(
    entry: &RuntimeMcpServerPolicyEntry,
    inspection: &McpServerInspection,
) -> bool {
    entry.server.capabilities.supports_resources
        || inspection.capabilities.get("resources").is_some()
}

fn build_mcp_tool_spec(
    entry: &RuntimeMcpServerPolicyEntry,
    tool: &claude_mcp::McpToolDescriptor,
) -> RuntimeMcpToolDescriptor {
    let qualified_name = build_mcp_tool_name(&entry.server.name, &tool.name);
    let description = tool.description.clone().unwrap_or_else(|| {
        format!(
            "Call the `{}` tool from the `{}` MCP server.",
            tool.name, entry.server.name
        )
    });
    let requires_permission = !annotation_hint_is_true(&tool.annotations, "readOnlyHint");
    let always_load = annotation_hint_is_true(&tool.annotations, "anthropic/alwaysLoad")
        || annotation_hint_is_true(&tool.annotations, "alwaysLoad");
    let mut search_hints = annotation_hint_strings(&tool.annotations, "anthropic/searchHint");
    search_hints.extend(annotation_hint_strings(&tool.annotations, "searchHint"));
    if let Some(title) = tool
        .title
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        search_hints.push(title.to_owned());
    }
    if let Some(search_hint) = annotation_hint_string(&tool.annotations, "anthropic/toolSearchHint")
    {
        search_hints.push(search_hint);
    }

    let spec_name = qualified_name.clone();
    crate::register_tool_runtime_hints(
        spec_name.clone(),
        ToolRuntimeHints {
            always_load,
            search_hints,
            destructive_hint: annotation_hint_bool(&tool.annotations, "destructiveHint"),
            open_world_hint: annotation_hint_bool(&tool.annotations, "openWorldHint"),
        },
    );
    RuntimeMcpToolDescriptor {
        tool_spec: ToolSpec {
            name: spec_name,
            protocol_name: build_mcp_tool_name(&entry.server.name, &tool.name),
            permission_tool_name: build_mcp_tool_name(&entry.server.name, &tool.name),
            description,
            requires_permission,
            input_schema: tool.input_schema.clone(),
        },
        server_name: entry.server.name.clone(),
        normalized_server_name: normalize_name_for_mcp(&entry.server.name),
        tool_name: tool.name.clone(),
        normalized_tool_name: normalize_name_for_mcp(&tool.name),
        server_config: entry.server.clone(),
        annotations: tool.annotations.clone(),
    }
}

fn build_mcp_prompt_command_descriptor(
    entry: &RuntimeMcpServerPolicyEntry,
    prompt: &claude_mcp::McpPromptDescriptor,
) -> RuntimeMcpPromptCommandDescriptor {
    RuntimeMcpPromptCommandDescriptor {
        command_name: build_mcp_prompt_command_name(&entry.server.name, &prompt.name),
        server_name: entry.server.name.clone(),
        normalized_server_name: normalize_name_for_mcp(&entry.server.name),
        prompt_name: prompt.name.clone(),
        description: prompt.description.clone().unwrap_or_default(),
        arg_names: prompt
            .arguments
            .iter()
            .map(|argument| argument.name.clone())
            .collect(),
        server_config: entry.server.clone(),
        prompt: prompt.clone(),
    }
}

async fn inspect_runtime_mcp_server(
    entry: &RuntimeMcpServerPolicyEntry,
) -> Result<McpServerInspection> {
    if let Some(snapshot_result) = snapshot_inspection_for_entry(entry) {
        return snapshot_result;
    }

    let cache_key = inspection_cache_key(entry);
    {
        let cache = RUNTIME_MCP_INSPECTION_CACHE.lock().await;
        if let Some(cached) = cache.get(&cache_key)
            && cached.server_config == entry.server
        {
            return Ok(cached.inspection.clone());
        }
    }

    let inspection = inspect_server(&entry.server, &McpClientInfo::default()).await?;
    let mut cache = RUNTIME_MCP_INSPECTION_CACHE.lock().await;
    cache.insert(
        cache_key,
        CachedInspection {
            server_config: entry.server.clone(),
            inspection: inspection.clone(),
        },
    );
    Ok(inspection)
}

pub async fn runtime_mcp_catalog() -> RuntimeMcpCatalog {
    let policy = current_tool_runtime_policy();
    let mut catalog = RuntimeMcpCatalog::default();
    let mut tool_map = BTreeMap::<String, RuntimeMcpToolDescriptor>::new();
    let mut prompt_map = BTreeMap::<String, RuntimeMcpPromptCommandDescriptor>::new();

    for entry in &policy.mcp_servers {
        if !entry.server.enabled {
            continue;
        }

        match inspect_runtime_mcp_server(entry).await {
            Ok(inspection) => {
                catalog.clients.push(RuntimeMcpClientDescriptor {
                    server_name: entry.server.name.clone(),
                    normalized_server_name: normalize_name_for_mcp(&entry.server.name),
                    instructions: inspection.instructions.clone(),
                    supports_resources: inspection_supports_resources(entry, &inspection),
                });

                for tool in entry.server.tool_policy.filter_tools(&inspection.tools) {
                    let descriptor = build_mcp_tool_spec(entry, &tool);
                    if !tool_allowed_by_policy(descriptor.qualified_name(), &policy) {
                        continue;
                    }

                    if let Some(existing) =
                        tool_map.insert(descriptor.qualified_name().to_owned(), descriptor.clone())
                    {
                        catalog.warnings.push(format!(
                            "Normalized MCP tool name collision for {} between {}:{} and {}:{}; keeping the later definition",
                            existing.qualified_name(),
                            existing.server_name,
                            existing.tool_name,
                            descriptor.server_name,
                            descriptor.tool_name
                        ));
                    }
                }

                for prompt in &inspection.prompts {
                    let descriptor = build_mcp_prompt_command_descriptor(entry, prompt);
                    if !tool_allowed_by_policy(&descriptor.command_name, &policy) {
                        continue;
                    }

                    if let Some(existing) =
                        prompt_map.insert(descriptor.command_name.clone(), descriptor.clone())
                    {
                        catalog.warnings.push(format!(
                            "Normalized MCP prompt command collision for {} between {}:{} and {}:{}; keeping the later definition",
                            existing.command_name,
                            existing.server_name,
                            existing.prompt_name,
                            descriptor.server_name,
                            descriptor.prompt_name
                        ));
                    }
                }
            }
            Err(error) => catalog.warnings.push(format!(
                "Failed to inspect MCP server {} from {}: {error}",
                entry.server.name,
                entry.config_path.display()
            )),
        }
    }

    catalog.clients.sort_by(|left, right| {
        left.server_name.cmp(&right.server_name).then_with(|| {
            left.normalized_server_name
                .cmp(&right.normalized_server_name)
        })
    });
    catalog.tools = tool_map.into_values().collect();
    catalog.tools.sort_by(|left, right| {
        left.qualified_name()
            .cmp(right.qualified_name())
            .then_with(|| left.server_name.cmp(&right.server_name))
            .then_with(|| left.tool_name.cmp(&right.tool_name))
    });
    catalog.prompts = prompt_map.into_values().collect();
    catalog.prompts.sort_by(|left, right| {
        left.command_name
            .cmp(&right.command_name)
            .then_with(|| left.server_name.cmp(&right.server_name))
            .then_with(|| left.prompt_name.cmp(&right.prompt_name))
    });
    catalog
}

#[must_use]
pub async fn runtime_mcp_tool_specs() -> Vec<ToolSpec> {
    runtime_mcp_catalog()
        .await
        .tools
        .into_iter()
        .map(|tool| tool.tool_spec)
        .collect()
}

pub async fn runtime_mcp_prompt_command_names() -> Vec<String> {
    runtime_mcp_catalog()
        .await
        .prompts
        .into_iter()
        .map(|prompt| prompt.command_name)
        .collect()
}

pub async fn resolve_runtime_mcp_tool(name: &str) -> Result<RuntimeMcpToolDescriptor> {
    runtime_mcp_catalog()
        .await
        .tools
        .into_iter()
        .find(|tool| tool.qualified_name() == name)
        .ok_or_else(|| anyhow!("MCP tool '{name}' is not available in the current runtime catalog"))
}

pub async fn runtime_mcp_prompt_commands() -> Vec<RuntimeMcpPromptCommandDescriptor> {
    runtime_mcp_catalog().await.prompts
}

pub async fn resolve_runtime_mcp_prompt_command(
    name: &str,
) -> Result<RuntimeMcpPromptCommandDescriptor> {
    runtime_mcp_catalog()
        .await
        .prompts
        .into_iter()
        .find(|prompt| prompt.command_name == name)
        .ok_or_else(|| {
            anyhow!("MCP prompt command '{name}' is not available in the current runtime catalog")
        })
}

pub async fn execute_runtime_mcp_prompt_command(
    name: &str,
    args: &str,
    context: &crate::ToolExecutionContext,
) -> Result<Vec<Value>> {
    let descriptor = resolve_runtime_mcp_prompt_command(name).await?;
    let arguments = descriptor
        .arg_names
        .iter()
        .zip(args.split_whitespace())
        .map(|(name, value)| (name.clone(), Value::String(value.to_owned())))
        .collect::<serde_json::Map<_, _>>();
    let response = claude_mcp::get_prompt(
        &descriptor.server_config,
        &McpClientInfo::default(),
        &descriptor.prompt_name,
        Value::Object(arguments),
    )
    .await?;

    let tool_results_dir = crate::mcp_tools::runtime_tool_results_dir(context);
    Ok(crate::mcp_tools::transform_mcp_prompt_messages(
        &response.result.messages,
        &descriptor.server_name,
        tool_results_dir.as_deref(),
    ))
}

pub async fn execute_runtime_mcp_tool(
    name: &str,
    input: &Value,
    context: &crate::ToolExecutionContext,
) -> Result<claude_core::ToolResult> {
    let descriptor = resolve_runtime_mcp_tool(name).await?;
    if !descriptor
        .server_config
        .tool_policy
        .is_tool_allowed(&descriptor.tool_name)
    {
        return Err(anyhow!(
            "MCP tool `{}` on server `{}` is not allowed by the server's tool policy",
            descriptor.tool_name,
            descriptor.server_name
        ));
    }
    let response = claude_mcp::call_tool(
        &descriptor.server_config,
        &McpClientInfo::default(),
        &descriptor.tool_name,
        input.clone(),
    )
    .await?;

    crate::mcp_tools::transform_mcp_tool_response(&response, context)
}

pub async fn clear_runtime_mcp_catalog_cache() {
    RUNTIME_MCP_INSPECTION_CACHE.lock().await.clear();
    crate::clear_tool_runtime_hints();
}

pub async fn invalidate_runtime_mcp_catalog_server(server_name: &str) {
    let mut cache = RUNTIME_MCP_INSPECTION_CACHE.lock().await;
    cache.retain(|_, cached| cached.inspection.server_name != server_name);
}

pub async fn handle_runtime_mcp_list_changed(server_name: &str, surface: McpListChangedSurface) {
    match surface {
        McpListChangedSurface::Tools | McpListChangedSurface::Prompts => {
            invalidate_runtime_mcp_catalog_server(server_name).await;
        }
        McpListChangedSurface::Resources => {
            // Resource listing is fetched by claude_mcp::list_resources on demand.
            // Keep tool/prompt cache intact, matching Claude Code's split
            // invalidation where resources/list_changed does not evict tools.
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::Mutex as StdMutex;

    use claude_mcp::{
        McpCapabilityMatrix, McpListChangedSurface, McpPeerInfo, McpServerConfig,
        McpServerInspection, McpToolDescriptor, McpToolPolicy, McpTransportConfig,
    };
    use once_cell::sync::Lazy;
    use serde_json::json;

    use super::{
        CachedInspection, RUNTIME_MCP_INSPECTION_CACHE, clear_runtime_mcp_catalog_cache,
        handle_runtime_mcp_list_changed, inspection_cache_key,
        invalidate_runtime_mcp_catalog_server,
    };
    use crate::{
        RuntimeMcpServerPolicyEntry, ToolRuntimePolicy, configure_tool_runtime_policy,
        current_tool_runtime_policy,
    };

    static MCP_CATALOG_TEST_MUTEX: Lazy<StdMutex<()>> = Lazy::new(|| StdMutex::new(()));

    fn policy_entry(server_name: &str, config_path: &str) -> RuntimeMcpServerPolicyEntry {
        RuntimeMcpServerPolicyEntry {
            origin_kind: "cwd".to_owned(),
            origin_name: "workspace".to_owned(),
            config_path: PathBuf::from(config_path),
            server: McpServerConfig {
                name: server_name.to_owned(),
                enabled: true,
                transport: McpTransportConfig::Stdio {
                    command: "python".to_owned(),
                    args: Vec::new(),
                    cwd: None,
                    env: BTreeMap::new(),
                },
                capabilities: McpCapabilityMatrix::default(),
                startup_timeout_secs: None,
                request_timeout_secs: None,
                metadata: BTreeMap::new(),
                oauth: None,
                tool_policy: claude_mcp::McpToolPolicy::default(),
            },
        }
    }

    fn cached_inspection(server_name: &str) -> CachedInspection {
        cached_inspection_with_tools(&policy_entry(server_name, "ignored"), &["search"])
    }

    fn cached_inspection_with_tools(
        entry: &RuntimeMcpServerPolicyEntry,
        tool_names: &[&str],
    ) -> CachedInspection {
        CachedInspection {
            server_config: entry.server.clone(),
            inspection: McpServerInspection {
                server_name: entry.server.name.clone(),
                protocol_version: "2025-03-26".to_owned(),
                server_info: Some(McpPeerInfo {
                    name: entry.server.name.clone(),
                    title: None,
                    version: None,
                }),
                capabilities: json!({"tools": {"listChanged": true}}),
                instructions: Some("instructions".to_owned()),
                tools: tool_names
                    .iter()
                    .map(|name| McpToolDescriptor {
                        name: (*name).to_owned(),
                        title: None,
                        description: None,
                        input_schema: json!({}),
                        annotations: json!({}),
                    })
                    .collect(),
                prompts: Vec::new(),
                resources: Vec::new(),
            },
        }
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn invalidate_runtime_mcp_catalog_server_removes_only_matching_server() {
        let _guard = MCP_CATALOG_TEST_MUTEX.lock().expect("test mutex");
        let first = policy_entry("test-invalidate-alpha", "test-invalidate-alpha.toml");
        let second = policy_entry("test-invalidate-beta", "test-invalidate-beta.toml");
        {
            let mut cache = RUNTIME_MCP_INSPECTION_CACHE.lock().await;
            cache.clear();
            cache.insert(
                inspection_cache_key(&first),
                cached_inspection("test-invalidate-alpha"),
            );
            cache.insert(
                inspection_cache_key(&second),
                cached_inspection("test-invalidate-beta"),
            );
        }

        invalidate_runtime_mcp_catalog_server("test-invalidate-alpha").await;

        let cache = RUNTIME_MCP_INSPECTION_CACHE.lock().await;
        assert!(!cache.contains_key(&inspection_cache_key(&first)));
        assert!(cache.contains_key(&inspection_cache_key(&second)));
        drop(cache);
        clear_runtime_mcp_catalog_cache().await;
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn list_changed_invalidates_tools_and_prompts_but_not_resources() {
        let _guard = MCP_CATALOG_TEST_MUTEX.lock().expect("test mutex");
        let entry = policy_entry("test-list-changed-alpha", "test-list-changed-alpha.toml");
        {
            let mut cache = RUNTIME_MCP_INSPECTION_CACHE.lock().await;
            cache.clear();
            cache.insert(
                inspection_cache_key(&entry),
                cached_inspection("test-list-changed-alpha"),
            );
        }

        handle_runtime_mcp_list_changed(
            "test-list-changed-alpha",
            McpListChangedSurface::Resources,
        )
        .await;
        assert!(
            RUNTIME_MCP_INSPECTION_CACHE
                .lock()
                .await
                .contains_key(&inspection_cache_key(&entry))
        );

        handle_runtime_mcp_list_changed("test-list-changed-alpha", McpListChangedSurface::Prompts)
            .await;
        assert!(
            !RUNTIME_MCP_INSPECTION_CACHE
                .lock()
                .await
                .contains_key(&inspection_cache_key(&entry))
        );
        clear_runtime_mcp_catalog_cache().await;
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn runtime_mcp_catalog_applies_server_tool_policy() {
        let _guard = MCP_CATALOG_TEST_MUTEX.lock().expect("test mutex");
        let original_policy = current_tool_runtime_policy();
        let mut entry = policy_entry("test-policy-server", "test-policy-server.toml");
        entry.server.tool_policy = McpToolPolicy::allow_only(["search"]);
        {
            let mut cache = RUNTIME_MCP_INSPECTION_CACHE.lock().await;
            cache.clear();
            cache.insert(
                inspection_cache_key(&entry),
                cached_inspection_with_tools(&entry, &["search", "delete"]),
            );
        }

        configure_tool_runtime_policy(ToolRuntimePolicy {
            mcp_servers: vec![entry],
            ..ToolRuntimePolicy::default()
        })
        .expect("configure policy");
        let catalog = super::runtime_mcp_catalog().await;

        assert_eq!(catalog.tools.len(), 1);
        assert_eq!(catalog.tools[0].tool_name, "search");
        configure_tool_runtime_policy(original_policy).expect("restore policy");
        clear_runtime_mcp_catalog_cache().await;
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn runtime_mcp_catalog_preserves_meta_hints_for_tool_search() {
        let _guard = MCP_CATALOG_TEST_MUTEX.lock().expect("test mutex");
        let original_policy = current_tool_runtime_policy();
        let entry = policy_entry("hint-server", "hint-server.toml");
        {
            let mut cache = RUNTIME_MCP_INSPECTION_CACHE.lock().await;
            cache.clear();
            cache.insert(
                inspection_cache_key(&entry),
                CachedInspection {
                    server_config: entry.server.clone(),
                    inspection: McpServerInspection {
                        server_name: entry.server.name.clone(),
                        protocol_version: "2025-03-26".to_owned(),
                        server_info: None,
                        capabilities: json!({"tools": {}}),
                        instructions: None,
                        tools: vec![McpToolDescriptor {
                            name: "lookup".to_owned(),
                            title: Some("Knowledge Lookup".to_owned()),
                            description: Some("Search project knowledge".to_owned()),
                            input_schema: json!({}),
                            annotations: json!({
                                "readOnlyHint": true,
                                "destructiveHint": false,
                                "openWorldHint": true,
                                "_meta": {
                                    "anthropic/alwaysLoad": true,
                                    "anthropic/searchHint": "docs semantic lookup"
                                }
                            }),
                        }],
                        prompts: Vec::new(),
                        resources: Vec::new(),
                    },
                },
            );
        }

        configure_tool_runtime_policy(ToolRuntimePolicy {
            mcp_servers: vec![entry],
            ..ToolRuntimePolicy::default()
        })
        .expect("configure policy");
        let catalog = super::runtime_mcp_catalog().await;

        let spec = &catalog.tools[0].tool_spec;
        assert!(!spec.requires_permission);
        assert!(spec.is_always_loaded());
        assert!(!spec.is_deferred());
        assert_eq!(spec.destructive_hint(), Some(false));
        assert_eq!(spec.open_world_hint(), Some(true));
        let terms = spec.tool_search_terms();
        assert!(terms.contains(&"docs".to_owned()), "{terms:?}");
        assert!(terms.contains(&"semantic".to_owned()), "{terms:?}");
        assert!(terms.contains(&"lookup".to_owned()), "{terms:?}");

        configure_tool_runtime_policy(original_policy).expect("restore policy");
        clear_runtime_mcp_catalog_cache().await;
    }
}
