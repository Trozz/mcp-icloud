//! create_event, update_event and delete_event.

use super::{CalendarServer, json_result, run};
use crate::caldav::client::Precondition;
use crate::caldav::discovery::{Account, CalendarInfo};
use crate::error::{self, CalError};
use crate::ical::build::{
    EventPatch, Freq, NewAttendee, NewEvent, Recurrence, all_day_range, apply_patch_as,
    new_resource, set_attendees,
};
use crate::ical::line::Component;
use crate::ical::model::{EventData, When};
use crate::ical::parse::{event_from_component, master, master_mut, parse_resource};
use crate::ical::recur::{
    exclude_occurrence, find_occurrence, override_mut, split_series, truncate_series,
};
use crate::tz::parse_date;
use crate::view::{event_fields, occurrence_in, occurrence_out, until_in, zoned};
use chrono::{Duration, Utc};
use chrono_tz::Tz;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::schemars::JsonSchema;
use rmcp::{ErrorData as McpError, tool, tool_router};
use serde::Deserialize;
use serde_json::{Value, json};
use url::Url;

const INVITES_DISABLED: &str =
    "invitations are disabled on this server (ICLOUD_ALLOW_INVITES=false)";

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct RecurrenceParams {
    /// daily, weekly, monthly or yearly.
    pub freq: String,
    /// Repeat every N periods. Default 1.
    pub interval: Option<u16>,
    /// Days as MO..SU; for monthly or yearly also with an ordinal such as 2TU (second Tuesday) or -1FR (last Friday).
    #[serde(default)]
    pub by_day: Vec<String>,
    /// Days of the month, 1 to 31 or -1 (last day).
    #[serde(default)]
    pub by_month_day: Vec<i8>,
    /// Last date (YYYY-MM-DD, inclusive) or date-time. Not together with count.
    pub until: Option<String>,
    /// Number of occurrences. Not together with until.
    pub count: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct AttendeeParams {
    pub email: String,
    pub name: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CreateEventParams {
    /// Calendar name or id from list_calendars.
    pub calendar: String,
    pub summary: String,
    /// Timed event start: RFC 3339, or without offset in the active zone. Use start_date for all-day events.
    pub start: Option<String>,
    /// Timed event end (exclusive), same format. Required with start.
    pub end: Option<String>,
    /// All-day event first day, YYYY-MM-DD.
    pub start_date: Option<String>,
    /// All-day event last day, YYYY-MM-DD, inclusive. Default: start_date.
    pub end_date: Option<String>,
    pub location: Option<String>,
    pub description: Option<String>,
    /// IANA zone for reading and storing the times. Default: the active zone.
    pub timezone: Option<String>,
    pub recurrence: Option<RecurrenceParams>,
    /// Alerts, in minutes before the start (0 to 40320).
    pub alerts_minutes_before: Option<Vec<i64>>,
    /// People to invite. Only when the server allows invitations; iCloud emails them.
    pub attendees: Option<Vec<AttendeeParams>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct UpdateEventParams {
    /// event_id from list_events.
    pub event_id: String,
    /// series (default), occurrence, or this_and_following. The last two need occurrence_start.
    pub scope: Option<String>,
    /// occurrence_start from list_events, for scope occurrence or this_and_following.
    pub occurrence_start: Option<String>,
    pub summary: Option<String>,
    /// New description; an empty string removes it.
    pub description: Option<String>,
    /// New location; an empty string removes it.
    pub location: Option<String>,
    /// New timed start. With only a new start, the duration is kept.
    pub start: Option<String>,
    /// New timed end (exclusive).
    pub end: Option<String>,
    /// New all-day first day, YYYY-MM-DD.
    pub start_date: Option<String>,
    /// New all-day last day, YYYY-MM-DD, inclusive.
    pub end_date: Option<String>,
    /// IANA zone for reading and storing new times. Default: the active zone.
    pub timezone: Option<String>,
    /// New repeat rule (series or this_and_following only).
    pub recurrence: Option<RecurrenceParams>,
    /// Replaces all alerts, in minutes before the start. [] removes them.
    pub alerts_minutes_before: Option<Vec<i64>>,
    /// Replaces the invited people. Only when the server allows invitations; [] removes everyone.
    pub attendees: Option<Vec<AttendeeParams>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct DeleteEventParams {
    /// event_id from list_events.
    pub event_id: String,
    /// series (default), occurrence, or this_and_following. The last two need occurrence_start.
    pub scope: Option<String>,
    /// occurrence_start from list_events, for scope occurrence or this_and_following.
    pub occurrence_start: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Scope {
    Series,
    Occurrence,
    ThisAndFollowing,
}

impl Scope {
    fn parse(s: Option<&str>) -> error::Result<Self> {
        match s.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            None | Some("series") => Ok(Scope::Series),
            Some("occurrence") => Ok(Scope::Occurrence),
            Some("this_and_following") => Ok(Scope::ThisAndFollowing),
            Some(other) => Err(CalError::invalid(format!(
                "scope must be series, occurrence or this_and_following, not {other:?}"
            ))),
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            Scope::Series => "series",
            Scope::Occurrence => "occurrence",
            Scope::ThisAndFollowing => "this_and_following",
        }
    }
}

/// Start/end input: timed (`start`/`end`) or all-day (`start_date`/`end_date`, inclusive),
/// never mixed. Returns DTSTART and the exclusive DTEND values that were given.
fn span_in(
    start: Option<&str>,
    end: Option<&str>,
    start_date: Option<&str>,
    end_date: Option<&str>,
    zone: Tz,
) -> error::Result<(Option<When>, Option<When>)> {
    let timed = start.is_some() || end.is_some();
    let dated = start_date.is_some() || end_date.is_some();
    if timed && dated {
        return Err(CalError::invalid(
            "use start/end for timed events or start_date/end_date for all-day events, not both",
        ));
    }
    if dated {
        return match (start_date, end_date) {
            (Some(s), Some(e)) => {
                let (s, e) = all_day_range(parse_date(s)?, parse_date(e)?)?;
                Ok((Some(s), Some(e)))
            }
            (Some(s), None) => Ok((Some(When::Date(parse_date(s)?)), None)),
            (None, Some(e)) => Ok((None, Some(When::Date(parse_date(e)? + Duration::days(1))))),
            (None, None) => Ok((None, None)),
        };
    }
    Ok((
        start.map(|s| zoned(s, zone)).transpose()?,
        end.map(|e| zoned(e, zone)).transpose()?,
    ))
}

fn recurrence_in(p: &RecurrenceParams, all_day: bool, zone: Tz) -> error::Result<Recurrence> {
    Ok(Recurrence {
        freq: Freq::parse(&p.freq)?,
        interval: p.interval,
        by_day: p.by_day.clone(),
        by_month_day: p.by_month_day.clone(),
        until: p
            .until
            .as_deref()
            .map(|u| until_in(u, all_day, zone))
            .transpose()?,
        count: p.count,
    })
}

fn attendees_in(list: &[AttendeeParams]) -> Vec<NewAttendee> {
    list.iter()
        .map(|a| NewAttendee {
            email: a.email.trim().to_string(),
            name: a.name.clone(),
        })
        .collect()
}

fn new_uid() -> String {
    uuid::Uuid::new_v4().to_string().to_uppercase()
}

fn require_etag(etag: Option<String>) -> error::Result<String> {
    etag.ok_or_else(|| {
        CalError::Upstream("the server sent no ETag; refusing to write without one".into())
    })
}

fn writable(cal: &CalendarInfo) -> error::Result<()> {
    if cal.writable {
        return Ok(());
    }
    Err(CalError::ReadOnly(format!("calendar {:?}", cal.name)))
}

fn occurrence_needed(scope: Scope, occurrence_start: Option<&str>) -> error::Result<Option<&str>> {
    match (scope, occurrence_start) {
        (Scope::Series, _) => Ok(None),
        (_, Some(o)) => Ok(Some(o)),
        (_, None) => Err(CalError::invalid(format!(
            "scope {} needs occurrence_start from list_events",
            scope.as_str()
        ))),
    }
}

/// A resource fetched for writing.
pub(crate) struct Editing {
    pub url: Url,
    pub calendar: CalendarInfo,
    pub etag: String,
    pub ical: Component,
    pub master: EventData,
}

impl CalendarServer {
    pub(crate) async fn load(&self, account: &Account, event_id: &str) -> error::Result<Editing> {
        let (url, cal) = account.resolve_event(event_id)?;
        writable(cal)?;
        let fetched = self.dir.client().get(&url).await?;
        let etag = require_etag(fetched.etag)?;
        let ical = parse_resource(&fetched.body)?;
        let m = master(&ical)
            .ok_or_else(|| CalError::Upstream("the event has no main VEVENT".into()))?;
        let master = event_from_component(m, &account.me)?;
        Ok(Editing {
            url,
            calendar: cal.clone(),
            etag,
            ical,
            master,
        })
    }

    /// Invitation rules for changing or deleting an existing event.
    fn guard(&self, ev: &EventData, attendees_change: bool) -> error::Result<()> {
        if self.settings.allow_invites {
            return Ok(());
        }
        if attendees_change {
            return Err(CalError::invalid(INVITES_DISABLED));
        }
        if ev.i_organize() && ev.has_other_attendees() {
            return Err(CalError::invalid(format!(
                "this event has attendees, so changing it would email them; {INVITES_DISABLED}"
            )));
        }
        Ok(())
    }

    /// An invitation from someone else: only my own alerts may change.
    fn guard_received(
        ev: &EventData,
        patch: &EventPatch,
        attendees_change: bool,
    ) -> error::Result<()> {
        let Some(org) = ev.organizer.as_ref().filter(|o| !o.is_me) else {
            return Ok(());
        };
        let only_alerts = EventPatch {
            alerts_minutes_before: None,
            ..patch.clone()
        }
        .is_empty();
        if only_alerts && !attendees_change {
            return Ok(());
        }
        Err(CalError::invalid(format!(
            "this is an invitation from {}; only alerts_minutes_before can be changed (use respond_to_invitation to reply)",
            org.email
        )))
    }

    /// Applies a field patch and an optional attendee list to one VEVENT.
    fn edit(
        &self,
        vevent: &mut Component,
        patch: &EventPatch,
        attendees: Option<&[AttendeeParams]>,
        me: &crate::ical::model::MyAddresses,
        zone: Tz,
    ) -> error::Result<()> {
        let now = Utc::now();
        if !patch.is_empty() {
            apply_patch_as(vevent, patch, me, now, zone)?;
        }
        if let Some(list) = attendees {
            set_attendees(vevent, &attendees_in(list), &self.settings.email, me)?;
            if patch.is_empty() {
                crate::ical::build::touch(vevent, now, true);
            }
        }
        Ok(())
    }

    async fn create(&self, p: CreateEventParams) -> error::Result<CallToolResult> {
        let zone = self.zone(p.timezone.as_deref())?;
        let attendees = p.attendees.as_deref().unwrap_or_default();
        if !attendees.is_empty() && !self.settings.allow_invites {
            return Err(CalError::invalid(INVITES_DISABLED));
        }
        let (start, end) = span_in(
            p.start.as_deref(),
            p.end.as_deref(),
            p.start_date.as_deref(),
            p.end_date.as_deref(),
            zone,
        )?;
        let start = start.ok_or_else(|| {
            CalError::invalid(
                "give start and end for a timed event, or start_date for an all-day event",
            )
        })?;
        let end = match (end, &start) {
            (Some(e), _) => e,
            (None, When::Date(d)) => When::Date(*d + Duration::days(1)),
            (None, _) => return Err(CalError::invalid("end is required with start")),
        };
        let recurrence = p
            .recurrence
            .as_ref()
            .map(|r| recurrence_in(r, start.is_date(), zone))
            .transpose()?;
        let account = self.dir.account().await?;
        let cal = account.calendar(&p.calendar)?;
        writable(cal)?;
        let uid = new_uid();
        let ev = NewEvent {
            summary: p.summary,
            start,
            end,
            location: p.location,
            description: p.description,
            recurrence,
            alerts_minutes_before: p.alerts_minutes_before.unwrap_or_default(),
            attendees: attendees_in(attendees),
            organizer_email: Some(self.settings.email.clone()),
        };
        let ical = new_resource(&ev, &uid, Utc::now(), zone)?;
        let url = Account::new_event_url(cal, &uid)?;
        self.dir
            .client()
            .put(&url, ical.to_ics(), Precondition::IfNoneMatch)
            .await?;
        let data = event_from_component(
            master(&ical).expect("new resources have a VEVENT"),
            &account.me,
        )?;
        let mut out = serde_json::Map::new();
        out.insert("created".into(), json!(true));
        out.insert("event_id".into(), json!(Account::event_id(&url)));
        out.insert("calendar".into(), json!(cal.name));
        out.extend(event_fields(&data, &data.start, &data.end, zone));
        Ok(json_result(&Value::Object(out)))
    }

    async fn update(&self, p: UpdateEventParams) -> error::Result<CallToolResult> {
        let zone = self.zone(p.timezone.as_deref())?;
        let scope = Scope::parse(p.scope.as_deref())?;
        let occurrence = occurrence_needed(scope, p.occurrence_start.as_deref())?;
        let (start, end) = span_in(
            p.start.as_deref(),
            p.end.as_deref(),
            p.start_date.as_deref(),
            p.end_date.as_deref(),
            zone,
        )?;
        let account = self.dir.account().await?;
        let mut e = self.load(&account, &p.event_id).await?;
        let all_day = start
            .as_ref()
            .map_or(e.master.start.is_date(), When::is_date);
        let patch = EventPatch {
            summary: p.summary,
            description: p.description,
            location: p.location,
            start,
            end,
            recurrence: p
                .recurrence
                .as_ref()
                .map(|r| recurrence_in(r, all_day, zone))
                .transpose()?,
            alerts_minutes_before: p.alerts_minutes_before,
        };
        let attendees = p.attendees.as_deref();
        if patch.is_empty() && attendees.is_none() {
            return Err(CalError::invalid(
                "nothing to change: give at least one field",
            ));
        }
        if scope == Scope::Occurrence && patch.recurrence.is_some() {
            return Err(CalError::invalid(
                "a single occurrence cannot have its own recurrence; use scope series or this_and_following",
            ));
        }
        self.guard(&e.master, attendees.is_some())?;
        Self::guard_received(&e.master, &patch, attendees.is_some())?;
        let client = self.dir.client();
        let mut out = serde_json::Map::new();
        out.insert("updated".into(), json!(true));
        out.insert("scope".into(), json!(scope.as_str()));
        out.insert("event_id".into(), json!(Account::event_id(&e.url)));
        let changed = match occurrence {
            None => {
                let m = master_mut(&mut e.ical).expect("checked in load");
                self.edit(m, &patch, attendees, &account.me, zone)?;
                event_from_component(m, &account.me)?
            }
            Some(input) if scope == Scope::Occurrence => {
                let rid = find_occurrence(&e.ical, &occurrence_in(input, zone)?, zone)?;
                out.insert("occurrence_start".into(), json!(occurrence_out(&rid, zone)));
                let o = override_mut(&mut e.ical, &rid, zone)?;
                self.edit(o, &patch, attendees, &account.me, zone)?;
                event_from_component(o, &account.me)?
            }
            Some(input) => {
                let rid = find_occurrence(&e.ical, &occurrence_in(input, zone)?, zone)?;
                let uid = new_uid();
                let split = split_series(&mut e.ical, &rid, &uid, Utc::now(), zone)?;
                let mut series = split.new_series;
                let m = master_mut(&mut series)
                    .ok_or_else(|| CalError::Internal("split series has no VEVENT".into()))?;
                self.edit(m, &patch, attendees, &account.me, zone)?;
                let data = event_from_component(m, &account.me)?;
                let new_url = Account::new_event_url(&e.calendar, &uid)?;
                let new_etag = client
                    .put(&new_url, series.to_ics(), Precondition::IfNoneMatch)
                    .await?;
                let old = if split.old_has_occurrences {
                    client
                        .put(
                            &e.url,
                            e.ical.to_ics(),
                            Precondition::IfMatch(e.etag.clone()),
                        )
                        .await
                        .map(|_| ())
                } else {
                    client.delete(&e.url, &e.etag).await
                };
                if let Err(err) = old {
                    if let Some(tag) = new_etag {
                        let _ = client.delete(&new_url, &tag).await;
                    }
                    return Err(err);
                }
                out.insert("new_event_id".into(), json!(Account::event_id(&new_url)));
                out.insert(
                    "old_series_deleted".into(),
                    json!(!split.old_has_occurrences),
                );
                out.extend(event_fields(&data, &data.start, &data.end, zone));
                return Ok(json_result(&Value::Object(out)));
            }
        };
        client
            .put(&e.url, e.ical.to_ics(), Precondition::IfMatch(e.etag))
            .await?;
        out.extend(event_fields(&changed, &changed.start, &changed.end, zone));
        Ok(json_result(&Value::Object(out)))
    }

    async fn delete(&self, p: DeleteEventParams) -> error::Result<CallToolResult> {
        let zone = self.settings.zones.active();
        let scope = Scope::parse(p.scope.as_deref())?;
        let occurrence = occurrence_needed(scope, p.occurrence_start.as_deref())?;
        let account = self.dir.account().await?;
        let mut e = self.load(&account, &p.event_id).await?;
        self.guard(&e.master, false)?;
        if !self.settings.allow_invites && e.master.organizer.as_ref().is_some_and(|o| !o.is_me) {
            return Err(CalError::invalid(format!(
                "this is an invitation from someone else; deleting it would send them a reply, and {INVITES_DISABLED}"
            )));
        }
        let client = self.dir.client();
        let now = Utc::now();
        let mut series_deleted = false;
        let mut out = serde_json::Map::new();
        match occurrence {
            None => {
                client.delete(&e.url, &e.etag).await?;
                series_deleted = true;
            }
            Some(input) => {
                let rid = find_occurrence(&e.ical, &occurrence_in(input, zone)?, zone)?;
                out.insert("occurrence_start".into(), json!(occurrence_out(&rid, zone)));
                let keep = if scope == Scope::Occurrence {
                    exclude_occurrence(&mut e.ical, &rid, now, zone)?;
                    true
                } else {
                    truncate_series(&mut e.ical, &rid, now, zone)?
                };
                if keep {
                    client
                        .put(&e.url, e.ical.to_ics(), Precondition::IfMatch(e.etag))
                        .await?;
                } else {
                    client.delete(&e.url, &e.etag).await?;
                    series_deleted = true;
                }
            }
        }
        out.insert("deleted".into(), json!(true));
        out.insert("scope".into(), json!(scope.as_str()));
        out.insert("event_id".into(), json!(Account::event_id(&e.url)));
        out.insert("series_deleted".into(), json!(series_deleted));
        Ok(json_result(&Value::Object(out)))
    }
}

#[tool_router(router = write_router, vis = "pub(crate)")]
impl CalendarServer {
    #[tool(
        description = "Create an event. Timed: start and end (RFC 3339, or without offset in the active zone). All-day: start_date and optional end_date (inclusive). Optional location, description, recurrence (freq, interval, by_day, by_month_day, until or count), alerts and attendees.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    pub async fn create_event(
        &self,
        Parameters(p): Parameters<CreateEventParams>,
    ) -> Result<CallToolResult, McpError> {
        tracing::debug!(calendar = %p.calendar, "create_event");
        run("create_event", self.create(p)).await
    }

    #[tool(
        description = "Change an event. Only the given fields change. scope series (default) edits the whole event or series; occurrence edits one occurrence (occurrence_start from list_events); this_and_following ends the series there and starts a new one with the changes.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    pub async fn update_event(
        &self,
        Parameters(p): Parameters<UpdateEventParams>,
    ) -> Result<CallToolResult, McpError> {
        tracing::debug!(event_id = %p.event_id, scope = ?p.scope, "update_event");
        run("update_event", self.update(p)).await
    }

    #[tool(
        description = "Delete an event. scope series (default) deletes the whole event or series; occurrence removes one occurrence; this_and_following ends the series before that occurrence.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    pub async fn delete_event(
        &self,
        Parameters(p): Parameters<DeleteEventParams>,
    ) -> Result<CallToolResult, McpError> {
        tracing::debug!(event_id = %p.event_id, scope = ?p.scope, "delete_event");
        run("delete_event", self.delete(p)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use chrono_tz::Europe::London;

    #[test]
    fn scopes_parse() {
        assert_eq!(Scope::parse(None).unwrap(), Scope::Series);
        assert_eq!(Scope::parse(Some("Occurrence")).unwrap(), Scope::Occurrence);
        assert_eq!(
            Scope::parse(Some("this_and_following")).unwrap(),
            Scope::ThisAndFollowing
        );
        assert_eq!(Scope::parse(Some("all")).unwrap_err().kind(), "validation");
    }

    #[test]
    fn spans_are_timed_or_all_day_never_both() {
        let (s, e) = span_in(None, None, Some("2026-12-24"), Some("2026-12-26"), London).unwrap();
        assert_eq!(
            s,
            Some(When::Date(NaiveDate::from_ymd_opt(2026, 12, 24).unwrap()))
        );
        assert_eq!(
            e,
            Some(When::Date(NaiveDate::from_ymd_opt(2026, 12, 27).unwrap()))
        );
        let (s, e) = span_in(Some("2026-10-20T09:00"), None, None, None, London).unwrap();
        assert_eq!(s.unwrap().tzid(), Some("Europe/London"));
        assert_eq!(e, None);
        let err = span_in(
            Some("2026-10-20T09:00"),
            None,
            Some("2026-10-20"),
            None,
            London,
        );
        assert_eq!(err.unwrap_err().kind(), "validation");
        let gap = span_in(Some("2026-03-29T01:30"), None, None, None, London).unwrap_err();
        assert!(gap.to_string().contains("daylight saving gap"), "{gap}");
    }

    #[test]
    fn occurrence_scopes_need_an_occurrence() {
        assert_eq!(occurrence_needed(Scope::Series, Some("x")).unwrap(), None);
        assert_eq!(
            occurrence_needed(Scope::Occurrence, None)
                .unwrap_err()
                .kind(),
            "validation"
        );
    }
}
