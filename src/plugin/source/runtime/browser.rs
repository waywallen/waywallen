use super::*;
use crate::plugin::browser_protocol::{BrowserCommand, BrowserEvent, BrowserRequest};
use mlua::LuaSerdeExt;

pub(crate) struct BrowserLoginBegin {
    pub operation_id: u64,
    pub browser: BrowserRequest,
    pub expires_in_ms: u64,
    pub title: LocalizedText,
    pub instruction: LocalizedText,
}

pub(crate) enum BrowserLoginOutcome {
    Waiting,
    Succeeded,
    Failed,
}

pub(crate) struct BrowserLoginUpdate {
    pub state: BrowserLoginOutcome,
    pub display_value: LocalizedText,
    pub error: LocalizedText,
    pub commands: Vec<BrowserCommand>,
}

impl LuaPluginRuntime {
    async fn browser_callback<R: FromLuaMulti>(
        &self,
        plugin: &str,
        function: &LuaFunction,
        args: impl IntoLuaMulti,
    ) -> Result<R> {
        if self.generation_state.load(Ordering::Acquire) != RUNTIME_ACTIVE {
            return Err(Error::InvalidArgument(
                "browser login plugin was reloaded".into(),
            ));
        }
        let result = self.call_callback_async(function, args).await;
        if self.generation_state.load(Ordering::Acquire) != RUNTIME_ACTIVE {
            return Err(Error::InvalidArgument(
                "browser login plugin was reloaded".into(),
            ));
        }
        self.persist_http_session(plugin)?;
        result.map_err(|error| Error::Internal(anyhow!(redact_secrets(&error.to_string()))))
    }

    pub(crate) async fn begin_browser_login(
        &mut self,
        plugin: &str,
        action: &str,
    ) -> Result<BrowserLoginBegin> {
        let info = self
            .plugin_infos
            .get(plugin)
            .ok_or_else(|| Error::SourcePluginNotFound(plugin.into()))?;
        if !info
            .actions
            .iter()
            .any(|a| a.id == action && a.kind == SourceActionKind::BrowserLogin)
        {
            return Err(Error::InvalidArgument(
                "action is not a browser login".into(),
            ));
        }
        let callbacks = self
            .callbacks
            .get(plugin)
            .ok_or_else(|| Error::SourcePluginNotFound(plugin.into()))?;
        let function: LuaFunction = self.lua.registry_value(
            callbacks
                .browserlogin_begin
                .as_ref()
                .ok_or_else(|| Error::InvalidArgument("browser login is unsupported".into()))?,
        )?;
        let ctx = self.build_remote_ctx(plugin)?;
        let result: LuaTable = self
            .browser_callback(plugin, &function, (ctx, action.to_string()))
            .await?;
        let browser: BrowserRequest =
            self.lua.from_value(result.get("browser")?).map_err(|_| {
                Error::InvalidArgument("invalid browserlogin.begin browser request".into())
            })?;
        browser.validate().map_err(Error::Internal)?;
        let key: LuaValue = result.get("key")?;
        if matches!(key, LuaValue::Nil) {
            return Err(Error::InvalidArgument(
                "browserlogin.begin key must not be nil".into(),
            ));
        }
        let expires_in_ms = result
            .get::<Option<u64>>("expires_in_ms")?
            .unwrap_or(600_000)
            .clamp(1000, 1_800_000);
        let title = Self::optional_localized_text(&result, "title", "browserlogin.begin")?;
        let instruction =
            Self::optional_localized_text(&result, "instruction", "browserlogin.begin")?;
        let operation_id = self.next_browser_operation_id;
        self.next_browser_operation_id = operation_id.wrapping_add(1).max(1);
        self.browser_operations
            .insert(operation_id, self.lua.create_registry_value(key)?);
        Ok(BrowserLoginBegin {
            operation_id,
            browser,
            expires_in_ms,
            title,
            instruction,
        })
    }

    pub(crate) async fn handle_browser_login(
        &mut self,
        plugin: &str,
        operation: u64,
        event: &BrowserEvent,
    ) -> Result<BrowserLoginUpdate> {
        let key = self.browser_operations.get(&operation).ok_or_else(|| {
            Error::InvalidArgument("browser login operation is no longer active".into())
        })?;
        let opaque: LuaValue = self.lua.registry_value(key)?;
        let callbacks = self
            .callbacks
            .get(plugin)
            .ok_or_else(|| Error::SourcePluginNotFound(plugin.into()))?;
        let function: LuaFunction = self.lua.registry_value(
            callbacks
                .browserlogin_handle
                .as_ref()
                .ok_or_else(|| Error::InvalidArgument("browser login is unsupported".into()))?,
        )?;
        let ctx = self.build_remote_ctx(plugin)?;
        let value = self.lua.to_value(event)?;
        let result: LuaTable = self
            .browser_callback(plugin, &function, (ctx, opaque, value))
            .await?;
        let state = match Self::require_string(&result, "state", "browserlogin.handle")?.as_str() {
            "waiting" => BrowserLoginOutcome::Waiting,
            "succeeded" => BrowserLoginOutcome::Succeeded,
            "failed" => BrowserLoginOutcome::Failed,
            _ => return Err(Error::InvalidArgument("invalid browser login state".into())),
        };
        let commands = match result.get::<LuaValue>("commands")? {
            LuaValue::Nil => vec![],
            value => self
                .lua
                .from_value::<Vec<BrowserCommand>>(value)
                .map_err(|_| Error::InvalidArgument("invalid browser login commands".into()))?,
        };
        if commands.len() > 16 {
            return Err(Error::InvalidArgument("too many browser commands".into()));
        }
        for command in &commands {
            command.validate().map_err(Error::Internal)?;
        }
        let display_value =
            Self::optional_localized_text(&result, "display_value", "browserlogin.handle")?;
        let error = Self::redact_localized_text(Self::optional_localized_text(
            &result,
            "error",
            "browserlogin.handle",
        )?);
        if !matches!(state, BrowserLoginOutcome::Waiting) {
            if let Some(key) = self.browser_operations.remove(&operation) {
                self.lua.remove_registry_value(key)?;
            }
            if matches!(state, BrowserLoginOutcome::Succeeded) {
                self.persist_state(plugin)?;
            }
        }
        Ok(BrowserLoginUpdate {
            state,
            display_value,
            error,
            commands,
        })
    }

    pub(crate) async fn cancel_browser_login(
        &mut self,
        plugin: &str,
        operation: u64,
    ) -> Result<()> {
        let Some(key) = self.browser_operations.remove(&operation) else {
            return Ok(());
        };
        if self.generation_state.load(Ordering::Acquire) != RUNTIME_ACTIVE {
            // A retired runtime can outlive its replacement while the browser closes.
            // Do not let its destructor overwrite the replacement HTTP session.
            if let Ok(snapshot) = self.http_cookie_store.snapshot() {
                self.http_cookie_store.mark_clean(snapshot.revision);
            }
            self.lua.remove_registry_value(key)?;
            return Ok(());
        }
        let opaque: LuaValue = self.lua.registry_value(&key)?;
        let callback = self
            .callbacks
            .get(plugin)
            .and_then(|c| c.browserlogin_cancel.as_ref());
        let result = if let Some(callback) = callback {
            let function: LuaFunction = self.lua.registry_value(callback)?;
            let ctx = self.build_remote_ctx(plugin)?;
            self.call_plugin_callback_async::<()>(plugin, &function, (ctx, opaque))
                .await
                .map_err(|e| Error::Internal(anyhow!(redact_secrets(&e.to_string()))))
        } else {
            Ok(())
        };
        self.lua.remove_registry_value(key)?;
        result
    }
}
