use crate::caldav::discovery::{Account, CalendarInfo, Directory};
use crate::caldav::xml::{DavResponse, calendar_query};
use crate::error::{self, CalError};
use crate::freebusy::{self, WorkingHours};
use crate::ical::model::{PartStat, When};
use crate::ical::parse::{event_from_component, master, overrides, parse_resource};
use crate::ical::recur::{MAX_OCCURRENCES, Occurrence, expand, find_occurrence};
use crate::tz::{ZoneState, format_in, parse_zone};
use crate::view::{bound, event_detail, event_fields, occurrence_in, occurrence_out, when_out};
use chrono::{DateTime, Duration, NaiveTime, Utc};
use chrono_tz::Tz;
use futures::{StreamExt, TryStreamExt};
use mcp_server_core::observability::content_logging_enabled;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::schemars::JsonSchema;
use rmcp::{ErrorData as McpError, ServerHandler, tool, tool_handler, tool_router};
use serde::Deserialize;
use serde_json::{Value, json};
use std::future::Future;
use std::sync::Arc;
use std::time::Instant;
use tracing::Instrument;

pub mod invite;
pub mod write;

pub const MAX_RANGE_DAYS: i64 = 366;
pub const MAX_CONCURRENT_REPORTS: usize = 4;
pub const MAX_FREE_SLOTS: usize = 100;
const DEFAULT_RANGE_DAYS: i64 = 7;

const INSTRUCTIONS: &str = "Tools for the iCloud calendars of one account. \
Times are shown in the active time zone (get_timezone); when the user says where they are, map the place to an IANA zone and call set_timezone. \
Inputs without a UTC offset are read in the active zone (or the call's timezone argument). \
Events are addressed by event_id from list_events; one occurrence of a recurring event also needs occurrence_start exactly as list_events returned it. \
All-day events use start_date and end_date (inclusive).";

/// Shared by every MCP session (the server is built per session and holds an `Arc` of this).
pub struct ToolSettings {
    pub zones: ZoneState,
    /// `ICLOUD_ALLOW_INVITES`: attendee changes and invitation replies.
    pub allow_invites: bool,
    /// The login email, written as ORGANIZER when inviting.
    pub email: String,
}

#[derive(Clone)]
pub struct CalendarServer {
    pub(crate) dir: Arc<Directory>,
    pub(crate) settings: Arc<ToolSettings>,
    tool_router: ToolRouter<CalendarServer>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ListEventsParams {
    /// Start of the range: YYYY-MM-DD (midnight) or a date-time. Default: today at 00:00.
    pub start: Option<String>,
    /// End of the range (exclusive), same formats. Default: 7 days after start. At most 366 days after start.
    pub end: Option<String>,
    /// Calendar names or ids from list_calendars. Default: all calendars.
    pub calendars: Option<Vec<String>>,
    /// Case-insensitive text to find in the summary, location or description.
    pub query: Option<String>,
    /// true: only invitations still waiting for my reply.
    pub needs_response: Option<bool>,
    /// IANA time zone for this call only, e.g. America/New_York. Default: the active zone.
    pub timezone: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct GetEventParams {
    /// event_id from list_events.
    pub event_id: String,
    /// For one occurrence of a recurring event: occurrence_start from list_events.
    pub occurrence_start: Option<String>,
    /// IANA time zone for this call only. Default: the active zone.
    pub timezone: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct WorkingHoursParams {
    /// Start of the working day, HH:MM.
    pub start: String,
    /// End of the working day, HH:MM.
    pub end: String,
    /// Working days as MO, TU, WE, TH, FR, SA, SU. Default: MO to FR.
    #[serde(default)]
    pub days: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct FreeTimeParams {
    /// Start of the search: YYYY-MM-DD or a date-time.
    pub start: String,
    /// End of the search (exclusive). At most 366 days after start.
    pub end: String,
    /// Minimum length of a free slot in minutes, 1 to 10080.
    pub duration_minutes: u32,
    /// Calendar names or ids to treat as busy. Default: all calendars.
    pub calendars: Option<Vec<String>>,
    /// Only search inside these hours. Default: whole days.
    pub working_hours: Option<WorkingHoursParams>,
    /// IANA time zone for this call only. Default: the active zone.
    pub timezone: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SetTimezoneParams {
    /// IANA time zone name such as America/New_York. null or omitted clears the override and returns to the configured default.
    pub zone: Option<String>,
}

/// One occurrence found in a calendar.
pub(crate) struct Found {
    pub calendar: CalendarInfo,
    pub event_id: String,
    pub occurrence: Occurrence,
}

pub(crate) fn json_result(v: &Value) -> CallToolResult {
    let text = serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string());
    CallToolResult::success(vec![ContentBlock::text(text)])
}

/// Runs one tool: span, metrics, and mapping of `CalError` to a tool error (`isError: true`)
/// whose text starts with the error kind, e.g. `conflict: ...`.
pub(crate) async fn run(
    tool: &'static str,
    fut: impl Future<Output = error::Result<CallToolResult>>,
) -> Result<CallToolResult, McpError> {
    let started = Instant::now();
    let result = fut.instrument(tracing::info_span!("tool", tool)).await;
    let outcome = if result.is_ok() { "ok" } else { "error" };
    metrics::counter!("mcp_tool_calls_total", "tool" => tool, "outcome" => outcome).increment(1);
    metrics::histogram!("mcp_tool_duration_seconds", "tool" => tool)
        .record(started.elapsed().as_secs_f64());
    tracing::info!(
        tool,
        outcome,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "tool call"
    );
    match result {
        Ok(r) => {
            if content_logging_enabled() {
                tracing::debug!(tool, result = ?r.content, "tool result");
            }
            Ok(r)
        }
        Err(e) => {
            tracing::warn!(tool, kind = e.kind(), error = %e, "tool failed");
            Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "{}: {e}",
                e.kind()
            ))]))
        }
    }
}

/// A range from optional bounds: default today 00:00 for 7 days; at most 366 days.
pub(crate) fn range(
    start: Option<&str>,
    end: Option<&str>,
    zone: Tz,
) -> error::Result<(DateTime<Utc>, DateTime<Utc>)> {
    let start = match start {
        Some(s) => bound(s, zone)?,
        None => {
            let today = Utc::now().with_timezone(&zone).date_naive();
            crate::tz::resolve_local(today.and_time(NaiveTime::MIN), zone)?
        }
    };
    let end = match end {
        Some(e) => bound(e, zone)?,
        None => start + Duration::days(DEFAULT_RANGE_DAYS),
    };
    if end <= start {
        return Err(CalError::invalid("end must be after start"));
    }
    if end - start > Duration::days(MAX_RANGE_DAYS) {
        return Err(CalError::invalid(format!(
            "the range is limited to {MAX_RANGE_DAYS} days; split it into smaller ranges"
        )));
    }
    Ok((start, end))
}

fn matches_query(o: &Occurrence, q: &str) -> bool {
    let q = q.to_lowercase();
    [&o.event.summary, &o.event.location, &o.event.description]
        .iter()
        .any(|f| f.as_deref().is_some_and(|t| t.to_lowercase().contains(&q)))
}

fn same_instant(a: &When, b: &When, zone: Tz) -> bool {
    a.instant(zone) == b.instant(zone)
}

impl CalendarServer {
    pub fn new(dir: Arc<Directory>, settings: Arc<ToolSettings>) -> Self {
        let mut tool_router = Self::read_router() + Self::write_router();
        if settings.allow_invites {
            tool_router += Self::invite_router();
        }
        Self {
            dir,
            settings,
            tool_router,
        }
    }

    /// Names of the registered tools, sorted.
    pub fn tool_names(&self) -> Vec<String> {
        self.tool_router
            .list_all()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect()
    }

    /// The call's zone: the `timezone` argument, else the active zone.
    pub(crate) fn zone(&self, timezone: Option<&str>) -> error::Result<Tz> {
        match timezone {
            Some(name) => parse_zone(name),
            None => Ok(self.settings.zones.active()),
        }
    }

    pub(crate) fn calendars<'a>(
        account: &'a Account,
        keys: Option<&[String]>,
    ) -> error::Result<Vec<&'a CalendarInfo>> {
        match keys {
            None => Ok(account.calendars.iter().collect()),
            Some([]) => Ok(account.calendars.iter().collect()),
            Some(keys) => keys.iter().map(|k| account.calendar(k)).collect(),
        }
    }

    /// One calendar-query. A 404 means the calendar list is stale: drop the cached discovery.
    async fn report(
        &self,
        cal: CalendarInfo,
        body: String,
    ) -> error::Result<(CalendarInfo, Vec<DavResponse>)> {
        let r = self.dir.client().report(&cal.url, body).await;
        if let Err(CalError::NotFound(_)) = &r {
            self.dir.invalidate().await;
        }
        r.map(|responses| (cal, responses))
    }

    /// Every occurrence in `[start, end)` across `calendars`, at most 4 REPORTs at a time.
    /// Resources iCloud returns that cannot be parsed are skipped with a warning.
    pub(crate) async fn occurrences(
        &self,
        account: &Account,
        calendars: Vec<&CalendarInfo>,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        zone: Tz,
    ) -> error::Result<(Vec<Found>, bool)> {
        let body = calendar_query(start, end);
        let mut reports = Vec::new();
        for cal in calendars {
            reports.push(self.report(cal.clone(), body.clone()));
        }
        let per_calendar: Vec<(CalendarInfo, Vec<DavResponse>)> = futures::stream::iter(reports)
            .buffer_unordered(MAX_CONCURRENT_REPORTS)
            .try_collect()
            .await?;
        let mut found = Vec::new();
        let mut truncated = false;
        for (cal, responses) in per_calendar {
            for r in responses {
                let Some(data) = r.props.calendar_data.as_deref() else {
                    continue;
                };
                let parsed = parse_resource(data)
                    .and_then(|ical| expand(&ical, &account.me, start, end, zone, MAX_OCCURRENCES));
                let expansion = match parsed {
                    Ok(x) => x,
                    Err(e) => {
                        tracing::warn!(href = %r.href, error = %e, "skipping an unreadable event");
                        continue;
                    }
                };
                truncated |= expansion.truncated;
                let event_id = account
                    .home
                    .join(&r.href)
                    .map(|u| Account::event_id(&u))
                    .unwrap_or(r.href.clone());
                for occurrence in expansion.occurrences {
                    found.push(Found {
                        calendar: cal.clone(),
                        event_id: event_id.clone(),
                        occurrence,
                    });
                }
            }
        }
        found.sort_by_key(|f| f.occurrence.start.instant(zone));
        Ok((found, truncated))
    }
}

#[tool_router(router = read_router)]
impl CalendarServer {
    #[tool(
        description = "List the calendars that hold events: id, name, colour and whether they can be written. Refreshes the cached calendar list.",
        annotations(read_only_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    pub async fn list_calendars(&self) -> Result<CallToolResult, McpError> {
        run("list_calendars", async {
            let account = self.dir.refresh().await?;
            let calendars: Vec<Value> = account
                .calendars
                .iter()
                .map(|c| {
                    json!({
                        "id": c.id,
                        "name": c.name,
                        "color": c.color,
                        "writable": c.writable,
                    })
                })
                .collect();
            Ok(json_result(&json!({ "calendars": calendars })))
        })
        .await
    }

    #[tool(
        description = "List event occurrences in a time range (default: today and the next 7 days), recurring events expanded, sorted by start. Optional filters: calendars, query text, needs_response. At most 500 occurrences; truncated is true when more exist.",
        annotations(read_only_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    pub async fn list_events(
        &self,
        Parameters(p): Parameters<ListEventsParams>,
    ) -> Result<CallToolResult, McpError> {
        tracing::debug!(args = ?p, "list_events");
        run("list_events", async {
            let zone = self.zone(p.timezone.as_deref())?;
            let (start, end) = range(p.start.as_deref(), p.end.as_deref(), zone)?;
            let account = self.dir.account().await?;
            let calendars = Self::calendars(&account, p.calendars.as_deref())?;
            let (found, mut truncated) = self
                .occurrences(&account, calendars, start, end, zone)
                .await?;
            let mut events: Vec<Value> = Vec::new();
            for f in found {
                let o = &f.occurrence;
                if p.query.as_deref().is_some_and(|q| !matches_query(o, q)) {
                    continue;
                }
                if p.needs_response == Some(true)
                    && o.event.my_status() != Some(&PartStat::NeedsAction)
                {
                    continue;
                }
                if events.len() == MAX_OCCURRENCES {
                    truncated = true;
                    break;
                }
                let mut item = serde_json::Map::new();
                item.insert("event_id".into(), json!(f.event_id));
                if let Some(rid) = &o.recurrence_id {
                    item.insert("occurrence_start".into(), json!(occurrence_out(rid, zone)));
                }
                item.insert("calendar".into(), json!(f.calendar.name));
                item.insert("calendar_id".into(), json!(f.calendar.id));
                item.extend(event_fields(&o.event, &o.start, &o.end, zone));
                events.push(Value::Object(item));
            }
            Ok(json_result(&json!({
                "timezone": zone.name(),
                "start": format_in(start, zone),
                "end": format_in(end, zone),
                "events": events,
                "truncated": truncated,
            })))
        })
        .await
    }

    #[tool(
        description = "Read one event in full: description, recurrence rule, excluded occurrences, attendees, organizer, alerts. Pass occurrence_start to read one occurrence of a recurring event.",
        annotations(read_only_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    pub async fn get_event(
        &self,
        Parameters(p): Parameters<GetEventParams>,
    ) -> Result<CallToolResult, McpError> {
        tracing::debug!(args = ?p, "get_event");
        run("get_event", async {
            let zone = self.zone(p.timezone.as_deref())?;
            let account = self.dir.account().await?;
            let (url, cal) = account.resolve_event(&p.event_id)?;
            let fetched = self.dir.client().get(&url).await?;
            let ical = parse_resource(&fetched.body)?;
            let m = master(&ical)
                .ok_or_else(|| CalError::Upstream("the event has no main VEVENT".into()))?;
            let master_data = event_from_component(m, &account.me)?;
            let mut out = serde_json::Map::new();
            out.insert("event_id".into(), json!(Account::event_id(&url)));
            out.insert("calendar".into(), json!(cal.name));
            out.insert("calendar_id".into(), json!(cal.id));
            let Some(input) = p.occurrence_start.as_deref() else {
                out.extend(event_detail(
                    &master_data,
                    &master_data.start,
                    &master_data.end,
                    zone,
                ));
                let changed: Vec<String> = overrides(&ical)
                    .filter_map(|o| event_from_component(o, &account.me).ok())
                    .filter_map(|o| o.recurrence_id.map(|r| when_out(&r, zone)))
                    .collect();
                if !changed.is_empty() {
                    out.insert("changed_occurrences".into(), json!(changed));
                }
                return Ok(json_result(&Value::Object(out)));
            };
            let rid = find_occurrence(&ical, &occurrence_in(input, zone)?, zone)?;
            out.insert("occurrence_start".into(), json!(occurrence_out(&rid, zone)));
            let changed = overrides(&ical)
                .filter_map(|o| event_from_component(o, &account.me).ok())
                .find(|o| {
                    o.recurrence_id
                        .as_ref()
                        .is_some_and(|r| same_instant(r, &rid, zone))
                });
            match changed {
                Some(o) => out.extend(event_detail(&o, &o.start, &o.end, zone)),
                None => {
                    let end = rid
                        .add(master_data.duration(zone))
                        .ok_or_else(|| CalError::invalid("the occurrence end is out of range"))?;
                    out.extend(event_detail(&master_data, &rid, &end, zone));
                }
            }
            Ok(json_result(&Value::Object(out)))
        })
        .await
    }

    #[tool(
        description = "Find free slots of at least duration_minutes between start and end, optionally only inside working hours. Events marked free, cancelled events and invitations I declined do not count as busy.",
        annotations(read_only_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    pub async fn find_free_time(
        &self,
        Parameters(p): Parameters<FreeTimeParams>,
    ) -> Result<CallToolResult, McpError> {
        tracing::debug!(args = ?p, "find_free_time");
        run("find_free_time", async {
            let zone = self.zone(p.timezone.as_deref())?;
            if !(1..=10_080).contains(&p.duration_minutes) {
                return Err(CalError::invalid(
                    "duration_minutes must be between 1 and 10080",
                ));
            }
            let (start, end) = range(Some(&p.start), Some(&p.end), zone)?;
            let hours = p
                .working_hours
                .as_ref()
                .map(|h| WorkingHours::parse(&h.start, &h.end, &h.days))
                .transpose()?;
            let account = self.dir.account().await?;
            let calendars = Self::calendars(&account, p.calendars.as_deref())?;
            let (found, expansion_truncated) = self
                .occurrences(&account, calendars, start, end, zone)
                .await?;
            let busy: Vec<freebusy::Interval> = found
                .iter()
                .map(|f| &f.occurrence)
                .filter(|o| {
                    !o.event.transparent
                        && !o.event.is_cancelled()
                        && o.event.my_status() != Some(&PartStat::Declined)
                })
                .map(|o| (o.start.instant(zone), o.end.instant(zone)))
                .collect();
            let windows = freebusy::windows(start, end, zone, hours.as_ref());
            let slots = freebusy::free_slots(
                &windows,
                busy,
                Duration::minutes(i64::from(p.duration_minutes)),
            );
            let truncated = expansion_truncated || slots.len() > MAX_FREE_SLOTS;
            let slots: Vec<Value> = slots
                .into_iter()
                .take(MAX_FREE_SLOTS)
                .map(|(s, e)| {
                    json!({
                        "start": format_in(s, zone),
                        "end": format_in(e, zone),
                        "minutes": (e - s).num_minutes(),
                    })
                })
                .collect();
            Ok(json_result(&json!({
                "timezone": zone.name(),
                "free": slots,
                "truncated": truncated,
            })))
        })
        .await
    }

    #[tool(
        description = "Show the active time zone, where it comes from (config or override) and the configured default.",
        annotations(read_only_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    pub async fn get_timezone(&self) -> Result<CallToolResult, McpError> {
        run("get_timezone", async { Ok(self.timezone_result()) }).await
    }

    #[tool(
        description = "Set the active time zone for all later calls, e.g. when the user says they are in another place (map the place to an IANA name such as America/New_York). Pass null to return to the configured default. The override is kept in memory until the server restarts.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub async fn set_timezone(
        &self,
        Parameters(p): Parameters<SetTimezoneParams>,
    ) -> Result<CallToolResult, McpError> {
        tracing::debug!(args = ?p, "set_timezone");
        run("set_timezone", async {
            let zone = p.zone.as_deref().map(parse_zone).transpose()?;
            self.settings.zones.set(zone);
            tracing::info!(zone = ?zone.map(|z| z.name()), "time zone override changed");
            Ok(self.timezone_result())
        })
        .await
    }
}

impl CalendarServer {
    fn timezone_result(&self) -> CallToolResult {
        let zones = &self.settings.zones;
        json_result(&json!({
            "timezone": zones.active().name(),
            "source": zones.source().as_str(),
            "default": zones.default_zone().name(),
            "now": format_in(Utc::now(), zones.active()),
        }))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for CalendarServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new(crate::config::SERVICE_NAME, crate::VERSION).with_icons(
                    mcp_server_core::service::icons(
                        include_bytes!("../../assets/icon.png"),
                        include_str!("../../assets/icon.svg"),
                    ),
                ),
            )
            .with_instructions(INSTRUCTIONS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use chrono_tz::Europe::London;

    #[test]
    fn ranges_default_to_a_week_from_local_midnight() {
        let (s, e) = range(None, None, London).unwrap();
        assert_eq!(e - s, Duration::days(7));
        let local = s.with_timezone(&London);
        assert_eq!(local.time(), NaiveTime::MIN);
    }

    #[test]
    fn ranges_are_limited_to_366_days() {
        assert!(range(Some("2026-01-01"), Some("2027-01-02"), London).is_ok());
        let e = range(Some("2026-01-01"), Some("2027-01-03"), London).unwrap_err();
        assert_eq!(e.kind(), "validation");
        assert!(e.to_string().contains("366 days"), "{e}");
        let e = range(Some("2026-01-02"), Some("2026-01-01"), London).unwrap_err();
        assert_eq!(e.kind(), "validation");
    }

    #[test]
    fn range_bounds_accept_dates_and_times() {
        let (s, e) = range(Some("2026-07-01"), Some("2026-07-01T12:00:00Z"), London).unwrap();
        assert_eq!(s, Utc.with_ymd_and_hms(2026, 6, 30, 23, 0, 0).unwrap());
        assert_eq!(e, Utc.with_ymd_and_hms(2026, 7, 1, 12, 0, 0).unwrap());
    }
}
