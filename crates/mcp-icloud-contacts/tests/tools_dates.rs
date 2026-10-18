mod support;
use chrono::{Datelike, Days, NaiveDate, Utc};
use chrono_tz::Tz;
use serde_json::json;
use support::cards::{contact, group};
use support::fake_carddav::{FakeCardDav, FakeState};
use support::tools::{error_text, json, params, server};

fn today_in(tz: Tz) -> NaiveDate {
    Utc::now().with_timezone(&tz).date_naive()
}

fn ymd(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

/// Cards with dates relative to `today`: a birthday in 3 days with a year, an anniversary in
/// 10 days, a birthday without a year in 40 days, a group and an unparseable card. The stored
/// years are leap years, so a 29 February date keeps its day.
async fn fake_for(today: NaiveDate) -> FakeCardDav {
    let soon = today + Days::new(3);
    let anniversary = today + Days::new(10);
    let later = today + Days::new(40);
    let mut state = FakeState::icloud_like();
    state.put(
        "bea.vcf",
        &contact(
            "BEA-1",
            "Bea",
            "Example",
            &[&format!("BDAY:{}", ymd(soon.with_year(1992).unwrap()))],
        ),
    );
    state.put(
        "cal.vcf",
        &contact(
            "CAL-1",
            "Cal",
            "Example",
            &[
                &format!(
                    "item1.X-ABDATE:{}",
                    ymd(anniversary.with_year(2016).unwrap())
                ),
                "item1.X-ABLabel:_$!<Anniversary>!$_",
                &format!("BDAY;X-APPLE-OMIT-YEAR=1604:1604-{}", later.format("%m-%d")),
            ],
        ),
    );
    state.put("club.vcf", &group("CLUB-1", "Club", &["BEA-1"]));
    state.put(
        "broken.vcf",
        "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:BROKEN-1\r\nBDAY:2000-01-01\r\n",
    );
    FakeCardDav::start(state).await
}

#[tokio::test]
async fn upcoming_dates_lists_the_next_birthdays_and_anniversaries() {
    let before = today_in(Tz::UTC);
    let fake = fake_for(before).await;
    let v = json(
        &server(&fake)
            .upcoming_dates(params(json!({})))
            .await
            .unwrap(),
    );
    let after = today_in(Tz::UTC);
    let today = NaiveDate::parse_from_str(v["today"].as_str().unwrap(), "%Y-%m-%d").unwrap();
    assert!(today == before || today == after, "{v}");
    let shift = (today - before).num_days();
    assert_eq!(v["days"], 30);
    let soon = before + Days::new(3);
    let anniversary = before + Days::new(10);
    let dates = v["dates"].as_array().unwrap();
    assert_eq!(dates.len(), 2, "{v}");
    assert_eq!(dates[0]["id"], "BEA-1");
    assert_eq!(dates[0]["display_name"], "Bea Example");
    assert_eq!(dates[0]["label"], "birthday");
    assert_eq!(dates[0]["date"], ymd(soon));
    assert_eq!(dates[0]["in_days"], 3 - shift);
    assert_eq!(dates[0]["years"], soon.year() - 1992);
    assert_eq!(dates[0]["is_me"], false);
    assert_eq!(dates[1]["id"], "CAL-1");
    assert_eq!(dates[1]["label"], "anniversary");
    assert_eq!(dates[1]["date"], ymd(anniversary));
    assert_eq!(dates[1]["years"], anniversary.year() - 2016);
}

#[tokio::test]
async fn a_longer_window_includes_dates_without_a_year() {
    let before = today_in(Tz::UTC);
    let fake = fake_for(before).await;
    let v = json(
        &server(&fake)
            .upcoming_dates(params(json!({"days": 60})))
            .await
            .unwrap(),
    );
    let dates = v["dates"].as_array().unwrap();
    assert_eq!(dates.len(), 3, "{v}");
    assert_eq!(dates[2]["id"], "CAL-1");
    assert_eq!(dates[2]["label"], "birthday");
    assert_eq!(dates[2]["date"], ymd(before + Days::new(40)));
    assert!(dates[2].get("years").is_none(), "{v}");
}

#[tokio::test]
async fn days_outside_1_to_366_is_a_validation_error() {
    let fake = fake_for(today_in(Tz::UTC)).await;
    let s = server(&fake);
    for days in [0, 367] {
        let e = error_text(
            &s.upcoming_dates(params(json!({"days": days})))
                .await
                .unwrap(),
        );
        assert_eq!(e, "validation: days must be between 1 and 366");
    }
}

#[tokio::test]
async fn today_follows_the_configured_time_zone() {
    let fake = fake_for(today_in(Tz::UTC)).await;
    for tz in [
        chrono_tz::Pacific::Kiritimati,
        chrono_tz::Pacific::Pago_Pago,
    ] {
        let before = today_in(tz);
        let v = json(
            &server(&fake)
                .with_timezone(tz)
                .upcoming_dates(params(json!({"days": 1})))
                .await
                .unwrap(),
        );
        let after = today_in(tz);
        let today = v["today"].as_str().unwrap();
        assert!(today == ymd(before) || today == ymd(after), "{tz}: {v}");
    }
}
