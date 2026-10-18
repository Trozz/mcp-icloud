//! Several services' MCP handlers served as one MCP server: the combined `/mcp` endpoint.
//!
//! Each service's tools and prompts appear as `<service>_<name>`; a call is routed by that
//! prefix to the service's handler with the original name. A forwarded `prompts/get` carries
//! [`ToolPrefix`] so the service names its tools as they are called here. [`DynHandler`] erases
//! a handler's type so handlers of different services fit in one list.

use crate::prompt::{ToolPrefix, not_found};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, GetPromptRequestParams, GetPromptResponse,
    Implementation, ListPromptsResult, ListToolsResult, PaginatedRequestParams, Prompt,
    PromptsCapability, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{ErrorData as McpError, ServerHandler};
use std::future::Future;
use std::pin::Pin;

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The parts of an MCP handler the combined endpoint uses, in a form that can be boxed.
pub trait ToolHandler: Send + Sync + 'static {
    fn info(&self) -> ServerConfig;
    fn tool(&self, name: &str) -> Option<Tool>;
    fn list(
        &self,
        context: RequestContext<RoleServer>,
    ) -> BoxFuture<'_, Result<Vec<Tool>, McpError>>;
    fn call(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> BoxFuture<'_, Result<CallToolResponse, McpError>>;
    fn prompts(
        &self,
        context: RequestContext<RoleServer>,
    ) -> BoxFuture<'_, Result<Vec<Prompt>, McpError>>;
    fn prompt(
        &self,
        request: GetPromptRequestParams,
        context: RequestContext<RoleServer>,
    ) -> BoxFuture<'_, Result<GetPromptResponse, McpError>>;
}

impl<S: ServerHandler> ToolHandler for S {
    fn info(&self) -> ServerConfig {
        self.get_info()
    }

    fn tool(&self, name: &str) -> Option<Tool> {
        self.get_tool(name)
    }

    /// Services list every tool in one page.
    fn list(
        &self,
        context: RequestContext<RoleServer>,
    ) -> BoxFuture<'_, Result<Vec<Tool>, McpError>> {
        Box::pin(async move { Ok(self.list_tools(None, context).await?.tools) })
    }

    fn call(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> BoxFuture<'_, Result<CallToolResponse, McpError>> {
        Box::pin(self.call_tool(request, context))
    }

    /// Services list every prompt in one page.
    fn prompts(
        &self,
        context: RequestContext<RoleServer>,
    ) -> BoxFuture<'_, Result<Vec<Prompt>, McpError>> {
        Box::pin(async move { Ok(self.list_prompts(None, context).await?.prompts) })
    }

    fn prompt(
        &self,
        request: GetPromptRequestParams,
        context: RequestContext<RoleServer>,
    ) -> BoxFuture<'_, Result<GetPromptResponse, McpError>> {
        Box::pin(self.get_prompt(request, context))
    }
}

/// A boxed service handler that is itself an MCP handler (tools and prompts).
pub struct DynHandler(Box<dyn ToolHandler>);

impl DynHandler {
    pub fn new(handler: impl ToolHandler) -> Self {
        Self(Box::new(handler))
    }
}

impl ServerHandler for DynHandler {
    fn get_info(&self) -> ServerConfig {
        self.0.info()
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.0.tool(name)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(self.0.list(context).await?))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        self.0.call(request, context).await
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, McpError> {
        Ok(ListPromptsResult::with_all_items(
            self.0.prompts(context).await?,
        ))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, McpError> {
        self.0.prompt(request, context).await
    }
}

/// `<service>_<tool>`.
pub fn prefixed_name(service: &str, tool: &str) -> String {
    format!("{service}_{tool}")
}

fn prefixed(service: &str, mut tool: Tool) -> Tool {
    tool.name = prefixed_name(service, &tool.name).into();
    tool
}

fn prefixed_prompt(service: &str, mut prompt: Prompt) -> Prompt {
    prompt.name = prefixed_name(service, &prompt.name);
    prompt
}

/// A service's "not found" for its own prompt name, reworded with the prefixed name the client
/// sent; any other error unchanged.
fn with_full_name(error: McpError, own: &str, full: &str) -> McpError {
    if error == not_found(own) {
        return not_found(full);
    }
    error
}

/// The service a prefixed tool or prompt name belongs to, and the name within the service.
/// Service names never contain `_`, so the first `_` ends the prefix.
fn split<'a>(services: &[&str], name: &'a str) -> Option<(usize, &'a str)> {
    let (service, tool) = name.split_once('_')?;
    let index = services.iter().position(|s| *s == service)?;
    (!tool.is_empty()).then_some((index, tool))
}

/// A short guide to the prefixes, then each service's own instructions under its name.
fn instructions(services: &[(&str, Option<String>)]) -> String {
    let prefixes: Vec<String> = services.iter().map(|(s, _)| format!("{s}_")).collect();
    let mut text = format!(
        "This server combines several iCloud services. Each tool name starts with its service: {}.",
        prefixes.join(", ")
    );
    for (service, own) in services {
        if let Some(own) = own.as_deref().filter(|t| !t.trim().is_empty()) {
            text.push_str(&format!("\n\n{service}:\n{}", own.trim()));
        }
    }
    text
}

/// One MCP server over several services' handlers.
pub struct Combined {
    info: ServerConfig,
    names: Vec<&'static str>,
    handlers: Vec<DynHandler>,
}

impl Combined {
    /// `services` in mount order, each already wrapped in its read-only and audit policy.
    /// Service names must not contain `_` (they are lowercase path segments).
    pub fn new(server_info: Implementation, services: Vec<(&'static str, DynHandler)>) -> Self {
        let own: Vec<(&str, Option<String>)> = services
            .iter()
            .map(|(name, h)| (*name, h.get_info().instructions))
            .collect();
        let mut capabilities = ServerCapabilities::builder().enable_tools().build();
        if services
            .iter()
            .any(|(_, h)| h.get_info().capabilities.prompts.is_some())
        {
            capabilities.prompts = Some(PromptsCapability::default());
        }
        let info = ServerConfig::new(capabilities)
            .with_server_info(server_info)
            .with_instructions(instructions(&own));
        let (names, handlers) = services.into_iter().unzip();
        Self {
            info,
            names,
            handlers,
        }
    }
}

impl ServerHandler for Combined {
    fn get_info(&self) -> ServerConfig {
        self.info.clone()
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        let (index, tool) = split(&self.names, name)?;
        self.handlers[index]
            .get_tool(tool)
            .map(|t| prefixed(self.names[index], t))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let mut tools = Vec::new();
        for (service, handler) in self.names.iter().zip(&self.handlers) {
            let listed = handler.list_tools(None, context.clone()).await?.tools;
            tools.extend(listed.into_iter().map(|t| prefixed(service, t)));
        }
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        mut request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let Some((index, tool)) = split(&self.names, &request.name) else {
            return Err(McpError::invalid_params("tool not found", None));
        };
        request.name = tool.to_string().into();
        self.handlers[index].call_tool(request, context).await
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, McpError> {
        let mut prompts = Vec::new();
        for (service, handler) in self.names.iter().zip(&self.handlers) {
            let listed = handler.list_prompts(None, context.clone()).await?.prompts;
            prompts.extend(listed.into_iter().map(|p| prefixed_prompt(service, p)));
        }
        Ok(ListPromptsResult::with_all_items(prompts))
    }

    /// Routes by prefix and tells the service, through [`ToolPrefix`], to name its tools with
    /// the same prefix. An unknown prompt is reported with the name the client sent.
    async fn get_prompt(
        &self,
        mut request: GetPromptRequestParams,
        mut context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, McpError> {
        let Some((index, prompt)) = split(&self.names, &request.name) else {
            return Err(not_found(&request.name));
        };
        let prompt = prompt.to_string();
        let full = std::mem::replace(&mut request.name, prompt.clone());
        context
            .extensions
            .insert(ToolPrefix(prefixed_name(self.names[index], "")));
        self.handlers[index]
            .get_prompt(request, context)
            .await
            .map_err(|e| with_full_name(e, &prompt, &full))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn names_are_prefixed_with_their_service() {
        assert_eq!(
            prefixed_name("mail", "search_messages"),
            "mail_search_messages"
        );
        let tool = Tool::new("list_events", "d", Arc::new(serde_json::Map::new()));
        assert_eq!(prefixed("calendar", tool).name, "calendar_list_events");
    }

    #[test]
    fn a_prefixed_name_routes_to_its_service_and_original_tool() {
        let services = ["mail", "calendar", "contacts"];
        assert_eq!(
            split(&services, "contacts_search_contacts"),
            Some((2, "search_contacts"))
        );
        assert_eq!(
            split(&services, "mail_get_message"),
            Some((0, "get_message"))
        );
        assert_eq!(split(&services, "search_contacts"), None);
        assert_eq!(split(&services, "reminders_list"), None);
        assert_eq!(split(&services, "mail_"), None);
        assert_eq!(split(&services, "mail"), None);
    }

    #[test]
    fn instructions_name_the_prefixes_and_keep_each_services_own() {
        let text = instructions(&[
            ("mail", Some("Read mail.\n".into())),
            ("calendar", None),
            ("contacts", Some("Find people.".into())),
        ]);
        assert_eq!(
            text,
            "This server combines several iCloud services. Each tool name starts with its service: \
mail_, calendar_, contacts_.\n\nmail:\nRead mail.\n\ncontacts:\nFind people."
        );
    }

    #[test]
    fn prompt_names_are_prefixed_with_their_service() {
        let prompt = Prompt::new("plan_week", Some("d"), None);
        assert_eq!(
            prefixed_prompt("calendar", prompt).name,
            "calendar_plan_week"
        );
        let services = ["mail", "calendar", "contacts"];
        assert_eq!(
            split(&services, "contacts_clean_up_duplicates"),
            Some((2, "clean_up_duplicates"))
        );
    }

    #[test]
    fn an_unknown_prompt_is_reported_with_the_name_the_client_sent() {
        let e = with_full_name(not_found("nope"), "nope", "contacts_nope");
        assert_eq!(e.code, rmcp::model::ErrorCode::INVALID_PARAMS);
        assert_eq!(e.message, "prompt 'contacts_nope' not found");
        let other = McpError::invalid_params("limit must be a string", None);
        assert_eq!(
            with_full_name(other.clone(), "nope", "contacts_nope"),
            other
        );
    }

    struct Bare(bool);

    impl ServerHandler for Bare {
        fn get_info(&self) -> ServerConfig {
            let mut capabilities = ServerCapabilities::builder().enable_tools().build();
            if self.0 {
                capabilities.prompts = Some(PromptsCapability::default());
            }
            ServerConfig::new(capabilities)
        }
    }

    fn combined_info(with_prompts: &[bool]) -> ServerConfig {
        let services = with_prompts
            .iter()
            .zip(["a", "b"])
            .map(|(p, name)| (name, DynHandler::new(Bare(*p))))
            .collect();
        Combined::new(Implementation::new("t", "1"), services).get_info()
    }

    #[test]
    fn prompts_are_offered_when_any_service_has_them() {
        let info = combined_info(&[false, true]);
        assert!(info.capabilities.prompts.is_some());
        assert!(info.capabilities.tools.is_some());
        let info = combined_info(&[false, false]);
        assert!(info.capabilities.prompts.is_none());
        assert!(info.capabilities.tools.is_some());
    }
}
