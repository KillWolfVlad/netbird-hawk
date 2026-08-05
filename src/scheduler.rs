use chrono::{DateTime, Days, LocalResult, NaiveDate, NaiveDateTime, TimeZone};

use crate::model::LocalTime;

pub const WALL_CLOCK_GUARD_SECONDS: u64 = 5;

pub fn circular_successor<'a>(profiles: &'a [String], active: &str) -> Option<&'a str> {
    if profiles.is_empty() {
        return None;
    }
    let index = profiles.iter().position(|profile| profile == active);
    let next = index.map_or(0, |index| (index + 1) % profiles.len());
    Some(profiles[next].as_str())
}

/// Resolves a wall-clock occurrence. Ambiguous times use the earliest instant;
/// nonexistent times advance minute-by-minute to the first valid local instant.
pub fn resolve_occurrence<Tz>(timezone: &Tz, date: NaiveDate, time: LocalTime) -> DateTime<Tz>
where
    Tz: TimeZone,
    Tz::Offset: Copy,
{
    let mut candidate = NaiveDateTime::new(date, time.0);
    for _ in 0..=(6 * 60) {
        match timezone.from_local_datetime(&candidate) {
            LocalResult::Single(value) => return value,
            LocalResult::Ambiguous(first, second) => return first.min(second),
            LocalResult::None => candidate += chrono::Duration::minutes(1),
        }
    }
    // Timezone transitions are far shorter than six hours. Keeping this as an
    // invariant makes the pure API total without silently inventing an instant.
    panic!("timezone contained a local-time gap longer than six hours")
}

/// Returns only the most recent due date, preventing historical catch-up bursts.
pub fn latest_due_date<Tz>(
    timezone: &Tz,
    now: DateTime<Tz>,
    activated_at: DateTime<Tz>,
    time: LocalTime,
    handled: Option<NaiveDate>,
) -> Option<NaiveDate>
where
    Tz: TimeZone,
    Tz::Offset: Copy,
{
    let today = now.date_naive();
    let today_occurrence = resolve_occurrence(timezone, today, time);
    let candidate = if today_occurrence <= now {
        today
    } else {
        today.checked_sub_days(Days::new(1))?
    };
    let occurrence = resolve_occurrence(timezone, candidate, time);
    (occurrence > activated_at && handled.is_none_or(|date| candidate > date)).then_some(candidate)
}

pub fn next_occurrence<Tz>(
    timezone: &Tz,
    now: DateTime<Tz>,
    activated_at: DateTime<Tz>,
    time: LocalTime,
    handled: Option<NaiveDate>,
) -> DateTime<Tz>
where
    Tz: TimeZone,
    Tz::Offset: Copy,
{
    let mut date = now.date_naive();
    loop {
        let candidate = resolve_occurrence(timezone, date, time);
        if candidate > now
            && candidate > activated_at
            && handled.is_none_or(|handled| date > handled)
        {
            return candidate;
        }
        date = date
            .checked_add_days(Days::new(1))
            .expect("calendar date overflow while scheduling");
    }
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveDate, TimeZone};
    use chrono_tz::America::New_York;

    use super::*;

    fn profiles() -> Vec<String> {
        ["alpha", "beta", "gamma"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn selects_circular_successors() {
        let profiles = profiles();
        assert_eq!(circular_successor(&profiles, "beta"), Some("gamma"));
        assert_eq!(circular_successor(&profiles, "gamma"), Some("alpha"));
        assert_eq!(circular_successor(&profiles, "manual"), Some("alpha"));
        assert_eq!(circular_successor(&profiles[..1], "manual"), Some("alpha"));
        assert_eq!(circular_successor(&[], "manual"), None);
    }

    #[test]
    fn activation_after_today_schedules_tomorrow() {
        let time: LocalTime = "08:00".parse().unwrap();
        let now = New_York.with_ymd_and_hms(2025, 2, 4, 9, 0, 0).unwrap();
        let next = next_occurrence(&New_York, now, now, time, None);
        assert_eq!(
            next.date_naive(),
            NaiveDate::from_ymd_opt(2025, 2, 5).unwrap()
        );
        assert_eq!(latest_due_date(&New_York, now, now, time, None), None);
    }

    #[test]
    fn spring_gap_uses_first_valid_instant() {
        let time: LocalTime = "02:30".parse().unwrap();
        let occurrence = resolve_occurrence(
            &New_York,
            NaiveDate::from_ymd_opt(2025, 3, 9).unwrap(),
            time,
        );
        assert_eq!(occurrence.format("%H:%M").to_string(), "03:00");
    }

    #[test]
    fn autumn_fold_uses_first_instant() {
        let time: LocalTime = "01:30".parse().unwrap();
        let occurrence = resolve_occurrence(
            &New_York,
            NaiveDate::from_ymd_opt(2025, 11, 2).unwrap(),
            time,
        );
        assert_eq!(occurrence.offset().to_string(), "EDT");
    }

    #[test]
    fn sleep_and_multi_day_downtime_yield_only_latest_due_date() {
        let time: LocalTime = "08:00".parse().unwrap();
        let activated = New_York.with_ymd_and_hms(2025, 1, 1, 7, 0, 0).unwrap();
        let now = New_York.with_ymd_and_hms(2025, 1, 10, 9, 0, 0).unwrap();
        let due = latest_due_date(&New_York, now, activated, time, None);
        assert_eq!(due, NaiveDate::from_ymd_opt(2025, 1, 10));
        assert_eq!(latest_due_date(&New_York, now, activated, time, due), None);
    }

    #[test]
    fn backward_clock_changes_do_not_replay_older_dates() {
        let time: LocalTime = "08:00".parse().unwrap();
        let activated = New_York.with_ymd_and_hms(2025, 1, 1, 7, 0, 0).unwrap();
        let now = New_York.with_ymd_and_hms(2025, 1, 4, 9, 0, 0).unwrap();
        let already_handled = NaiveDate::from_ymd_opt(2025, 1, 5).unwrap();
        assert_eq!(
            latest_due_date(&New_York, now, activated, time, Some(already_handled)),
            None
        );
        assert_eq!(
            next_occurrence(&New_York, now, activated, time, Some(already_handled)).date_naive(),
            NaiveDate::from_ymd_opt(2025, 1, 6).unwrap()
        );
    }

    #[test]
    fn timezone_change_recomputes_the_wall_clock_occurrence() {
        let time: LocalTime = "08:00".parse().unwrap();
        let new_york_now = New_York.with_ymd_and_hms(2025, 1, 4, 7, 0, 0).unwrap();
        let tokyo = chrono_tz::Asia::Tokyo;
        let tokyo_now = new_york_now.with_timezone(&tokyo);
        let activated_utc = chrono::Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        let new_york_next = next_occurrence(
            &New_York,
            new_york_now,
            activated_utc.with_timezone(&New_York),
            time,
            None,
        );
        let tokyo_next = next_occurrence(
            &tokyo,
            tokyo_now,
            activated_utc.with_timezone(&tokyo),
            time,
            None,
        );
        assert_ne!(
            new_york_next.with_timezone(&chrono::Utc),
            tokyo_next.with_timezone(&chrono::Utc)
        );
    }

    #[test]
    fn clock_change_recalculation_uses_current_wall_clock() {
        let time: LocalTime = "08:00".parse().unwrap();
        let activated = New_York.with_ymd_and_hms(2025, 1, 1, 7, 0, 0).unwrap();
        let before = New_York.with_ymd_and_hms(2025, 1, 2, 7, 59, 0).unwrap();
        let after = New_York.with_ymd_and_hms(2025, 1, 2, 8, 1, 0).unwrap();
        assert_eq!(
            latest_due_date(&New_York, before, activated, time, None),
            NaiveDate::from_ymd_opt(2025, 1, 1)
        );
        assert_eq!(
            latest_due_date(&New_York, after, activated, time, None),
            NaiveDate::from_ymd_opt(2025, 1, 2)
        );
    }
}
