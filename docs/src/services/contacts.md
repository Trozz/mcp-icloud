# Contacts

Endpoint `/contacts` (or `contacts_<tool>` under [`/mcp`](combined.md)). The contacts service reads and edits your iCloud address book over CardDAV, including groups (Lists in iOS Contacts).

## Tools

| Tool | Kind | What it does | Key parameters |
|---|---|---|---|
| `search_contacts` | read | Find contacts by name, nickname, organization, department, job title, email or related name (every word must match; case and accents are ignored), or by a phone number in any format. Without a query it lists everyone in name order | `query`, `group_id`, `limit` (1 to 100, default 25), `offset` |
| `get_contact` | read | Everything about one contact: names, organization, emails, phones, addresses, URLs, birthday, dates, related names, social profiles, instant messaging, note and groups | `id` |
| `get_my_contact` | read | Your own card (My Card in iOS Contacts), in full | none |
| `list_groups` | read | Every group with its id, name and number of members | none |
| `upcoming_dates` | read | Birthdays, anniversaries and other dates from today up to `days` ahead, soonest first, with the days from today and, when the year is known, the age or number of years | `days` (1 to 366, default 30) |
| `find_duplicate_contacts` | read | Groups of contacts that look like the same person: a shared email, a shared phone number or the same full name, with the reasons, best contact to keep first | none |
| `export_vcard` | read | A contact as vCard (`.vcf`) text, exactly as iCloud stores it, returned as a `text/vcard` resource with a summary | `id` |
| `create_contact` | write | Create a contact. Needs a given name, family name, display name or organization | contact fields (below) |
| `update_contact` | write | Change only the fields you pass; `""` clears one. A list you pass replaces that whole list | `id` plus contact fields |
| `delete_contact` | write | Delete a contact and remove it from every group. Your own card cannot be deleted | `id` |
| `merge_contacts` | write | Merge up to 10 contacts into one: missing details are added to the kept contact, group memberships move to it, and the others are deleted | `keep_id`, `merge_ids` |
| `copy_contact_photo` | write | Copy one contact's photo onto another, replacing its photo, for example before merging into a contact without one | `from_id`, `to_id` |
| `create_group` | write | Create a group with optional members | `name`, `member_ids` |
| `update_group` | write | Rename a group, add or remove members. Removing a contact from a group does not delete it | `id`, `name`, `add_ids`, `remove_ids` |
| `delete_group` | write | Delete a group. Its contacts are kept | `id` |

## Contact fields

`given_name`, `family_name`, `middle_name`, `prefix`, `suffix`, `display_name`, `nickname`, `organization`, `department`, `job_title`, `birthday`, `note`, and the lists `emails`, `phones`, `urls` (each `[{value, label, preferred}]`), `addresses` (`[{street, city, region, postcode, country, label, preferred}]`), `dates` (`[{date, label}]`) and `related` (`[{name, label}]`).

- Labels are plain words: `home`, `work`, `other`, `mobile`, `iPhone`, `main`, `pager`, `home fax`, `work fax`, `home page`, `anniversary`, relations such as `mother` or `friend`, or any custom text.
- Birthdays and dates are `YYYY-MM-DD`, or `--MM-DD` without a year.
- `display_name` defaults to the name parts joined, otherwise the organization.

Contacts and groups are addressed by `id`, as `search_contacts` and `list_groups` return it.

## Lossless edits

Only the lines a change touches are rewritten. Photos, Apple's own fields and anything this server does not show survive every edit, and entries you pass back exactly as `get_contact` returned them keep their original lines. Updates are conditional writes: if a contact changed in iCloud since it was read, the tool returns a `conflict` error instead of overwriting it.

## Upcoming dates

- "Today" is the current date in `CONTACTS_TIMEZONE` (default `UTC`).
- The window includes today and the last day: `days: 1` means today and tomorrow.
- A date on 29 February shows on 28 February in other years. Dates without a year (`--MM-DD`) have no age.
- Birthdays are labelled `birthday`; other dates keep their own label (`anniversary`, a custom text, or `date` without one).

## Duplicates and merging

- `find_duplicate_contacts` links contacts that share a normalised email address, a phone number (the same last 9 digits; numbers of 6 to 8 digits match on all their digits, shorter ones are ignored) or the same full name of at least two words. Linked contacts form one group. The first contact in each group (one with a photo, then the most details) is a sensible `keep_id`.
- `merge_contacts` checks everything before writing anything. It adds the emails, phones, URLs, addresses, dates and related names the kept contact lacks, fills its empty single fields, appends notes, moves group memberships to the kept contact, then deletes the merged contacts. Other fields of merged contacts (social profiles, instant messaging, categories, custom X- fields) are not kept. Your own card can be kept but never merged away.
- Photos are never copied by a merge. When a merged contact had a photo and the kept one has none, the result lists it under `photo_not_kept`; use `copy_contact_photo` first, or keep the contact with the photo.
- If a later step fails (a group update or a delete), earlier steps are not undone; the result lists what was not done.

## iCloud notes

- iCloud does not enforce `If-Match` on contact deletes, so deletes cannot be conditional. The server re-reads the address book at the start of every call and deletes immediately after, so only a change made on another device in those milliseconds could be lost.
- Deleting a contact in iCloud does not remove it from groups; the server does that itself.
- iCloud stores cards exactly as sent; Apple's own fields and folding are preserved.
- The server keeps an in-memory index of the address book and brings it up to date with a sync at the start of every call, so searches are fast and see changes made on your devices.
- Your credentials are only sent to `https` hosts under `icloud.com` (or the exact `CARDDAV_URL` host when overridden); redirects elsewhere are refused.
