# Calendar

Endpoint `/calendar` (or `calendar_<tool>` under [`/mcp`](combined.md)). The calendar service reads and edits your iCloud calendars over CalDAV.

## Tools

| Tool | Kind | What it does | Key parameters |
|---|---|---|---|
| `list_calendars` | read | Calendars that hold events: `id`, `name`, colour and whether they can be written | none |
| `list_events` | read | Event occurrences in a range, recurring events expanded, sorted by start. At most 500 occurrences; `truncated` is true when more exist | `start` (default today at 00:00), `end` (default 7 days after `start`, at most 366 days after it), `calendars`, `query`, `needs_response`, `timezone` |
| `get_event` | read | One event in full: description, recurrence (in words and as RRULE), excluded occurrences, attendees, organizer, alerts, URL | `event_id`, `occurrence_start`, `timezone` |
| `find_free_time` | read | Free slots of at least `duration_minutes`, optionally inside working hours. Events marked free, cancelled events and invitations you declined are not busy | `start`, `end`, `duration_minutes` (1 to 10080), `calendars`, `working_hours` (`start`, `end` as `HH:MM`, `days`), `timezone` |
| `get_timezone` | read | The active time zone, where it comes from, and the configured default | none |
| `set_timezone` | write | Set the active time zone for later calls, for example while you travel; `null` returns to the default. Kept in memory until a restart | `zone` |
| `create_event` | write | Create a timed or all-day event, optionally recurring, with alerts and (when allowed) attendees | `calendar`, `summary`, `start` and `end`, or `start_date` and `end_date` (inclusive), `location`, `description`, `timezone`, `recurrence`, `alerts_minutes_before`, `attendees` |
| `update_event` | write | Change only the fields you pass, for the whole series, one occurrence, or this and following occurrences | `event_id`, `scope` (`series`, `occurrence`, `this_and_following`), `occurrence_start`, and any field of `create_event` |
| `delete_event` | write | Delete the series, one occurrence, or this and following occurrences | `event_id`, `scope`, `occurrence_start` |
| `respond_to_invitation` | write | Accept, decline or tentatively accept an invitation; iCloud sends the reply to the organizer. Registered only with `ICLOUD_ALLOW_INVITES=true` | `event_id`, `response` (`accepted`, `declined`, `tentative`), `occurrence_start` |

## Prompts

Prompts are ready-made requests a client can offer you. They tell the model which tools to call and in what order; they change nothing by themselves.

| Prompt | Arguments | What it does |
|---|---|---|
| `plan_week` | `start` (`YYYY-MM-DD`, default today in the active time zone) | Lays out the seven days from `start`, points out overlapping and back-to-back events, lists invitations waiting for your reply and suggests free slots with `find_free_time`. Replies to an invitation only with the answer you give |

Arguments are text; a blank argument takes its default, and an invalid one (a `start` that is not a `YYYY-MM-DD` date) is refused. Replying to invitations needs `ICLOUD_ALLOW_INVITES=true`; without it the model is told to say so and skip that step.

## Addressing events

Events are addressed by `event_id` as `list_events` returns it. One occurrence of a recurring event is additionally addressed by `occurrence_start`, exactly as `list_events` returns it. Every change is a conditional write: if the event changed in iCloud since it was read, the tool returns a `conflict` error instead of overwriting it.

## Recurrence and alerts

- `recurrence` takes `freq` (`daily`, `weekly`, `monthly`, `yearly`), `interval`, `by_day` (`MO` to `SU`, with an ordinal such as `2TU` or `-1FR` for monthly and yearly), `by_month_day` (1 to 31 or -1), and either `until` or `count`.
- `alerts_minutes_before` sets alerts in minutes before the start (0 to 40320). On `update_event` it replaces all alerts, and `[]` removes them.

## Time zones

- Times are shown in the active zone, plus the event's own zone when it differs. Times you pass without an offset are read in the active zone; times with an offset are used as given. All-day events are dates and never shift.
- The active zone is `CALENDAR_TIMEZONE` (default `Europe/London`) unless overridden with `set_timezone`. The override lives in memory: a restart returns to `CALENDAR_TIMEZONE`.
- Every time-based tool also takes `timezone` for one call.
- A local time that does not exist (skipped by a daylight saving change) is rejected; an ambiguous one (a repeated hour) uses the earlier time.

## Invitations

With `ICLOUD_ALLOW_INVITES=false` (the default), attendees are always shown and preserved, but the server will not change them: attendee arguments are refused, `respond_to_invitation` is not registered, and events you organize that have attendees, and invitations from others (apart from your own alerts), cannot be changed or deleted, because iCloud would email the attendees.

With `ICLOUD_ALLOW_INVITES=true`, adding or removing attendees makes iCloud send invitations or cancellations, and `respond_to_invitation` sends your reply to the organizer.

## iCloud notes

- Reminders lists are not available: Apple no longer serves upgraded iCloud Reminders over CalDAV, so only event calendars appear.
- iCloud adds its own time zone definitions to stored events.
- Your credentials are only sent to `https` hosts under `icloud.com` (or the exact `CALDAV_URL` host when overridden); redirects elsewhere are refused.
