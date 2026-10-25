use super::*;

fn s(f: impl FnOnce(&mut String)) -> String {
    let mut o = String::new();
    f(&mut o);
    o
}

#[test]
fn sizes() {
    assert_eq!(s(|o| size(o, 0)), "0 B");
    assert_eq!(s(|o| size(o, 1023)), "1023 B");
    assert_eq!(s(|o| size(o, 1536)), "1.5 KB");
    assert_eq!(s(|o| size(o, 150 * 1024 * 1024)), "150 MB");
}

#[test]
fn decimal_sizes_count_by_thousands() {
    assert_eq!(s(|o| size_in(o, 999, true)), "999 B");
    assert_eq!(s(|o| size_in(o, 1500, true)), "1.5 kB");
    assert_eq!(s(|o| size_in(o, 1536, false)), "1.5 KB");
    assert_eq!(s(|o| size_in(o, 2_000_000_000, true)), "2.0 GB");
}

const UNIX: i64 = 1_792_891_860;

fn ticks(unix: i64) -> i64 {
    (unix + 11_644_473_600) * 10_000_000
}

#[test]
fn dates() {
    let ft = ticks(UNIX);
    assert_eq!(s(|o| datetime(o, ft, 0)), "2026-10-25 01:31");
    assert_eq!(s(|o| datetime(o, ft, 180)), "2026-10-25 04:31");
    assert_eq!(s(|o| datetime(o, ft, -120)), "2026-10-24 23:31");
    assert_eq!(s(|o| datetime(o, 0, 0)), "");
}

#[test]
fn system_pictures_follow_windows_patterns() {
    let secs = UNIX;
    assert_eq!(s(|o| picture(o, "dd.MM.yyyy", secs)), "25.10.2026");
    assert_eq!(s(|o| picture(o, "M/d/yyyy", secs)), "10/25/2026");
    assert_eq!(s(|o| picture(o, "h:mm tt", secs)), "1:31 AM");
    assert_eq!(s(|o| picture(o, "HH:mm:ss", secs + 13 * 3600)), "14:31:00");
    assert_eq!(s(|o| picture(o, "dddd, d MMMM yy", secs)), "Sunday, 25 October 26");
    assert_eq!(s(|o| picture(o, "yyyy'年'M'月' 'o''clock'", secs)), "2026年10月 o'clock");
    let style = Style { dates: DateFormat::System, system: ("d/M/yyyy".into(), "H:mm".into()), ..Default::default() };
    assert_eq!(s(|o| datetime_in(o, ticks(UNIX), 0, &style)), "25/10/2026 1:31");
}

#[test]
fn application_dates_are_relative_to_today() {
    let today = local_days(ticks(UNIX), 0);
    let style = Style { dates: DateFormat::Application, today: Some(today), ..Default::default() };
    let at = |days: i64| s(|o| datetime_in(o, ticks(UNIX - days * 86_400), 0, &style));
    assert_eq!(at(0), "Today 01:31");
    assert_eq!(at(1), "Yesterday 01:31");
    assert_eq!(at(3), "Thursday 01:31");
    assert_eq!(at(30), "25 Sep 01:31");
    assert_eq!(at(400), "20 Sep 2025");
    let unknown = Style { today: None, ..style.clone() };
    assert_eq!(s(|o| datetime_in(o, ticks(UNIX), 0, &unknown)), "25 Oct 2026");
    assert_eq!(s(|o| datetime_in(o, ticks(UNIX), 0, &Style::default())), "2026-10-25 01:31");
}

#[test]
fn russian_sizes_and_dates() {
    crate::i18n::set_lang(crate::i18n::Lang::Ru);
    assert_eq!(s(|o| size(o, 1023)), "1023 Б");
    assert_eq!(s(|o| size(o, 1536)), "1,5 КБ");
    assert_eq!(s(|o| size_in(o, 1500, true)), "1,5 кБ");
    assert_eq!(s(|o| size(o, 150 * 1024 * 1024)), "150 МБ");
    assert_eq!(s(|o| picture(o, "dddd, d MMMM yyyy", UNIX)), "воскресенье, 25 октября 2026");
    assert_eq!(s(|o| picture(o, "MMMM yyyy", UNIX)), "Октябрь 2026");
    assert_eq!(s(|o| picture(o, "ddd dd.MM.yy", UNIX)), "Вс 25.10.26");
    let today = local_days(ticks(UNIX), 0);
    let style = Style { dates: DateFormat::Application, today: Some(today), ..Default::default() };
    let at = |days: i64| s(|o| datetime_in(o, ticks(UNIX - days * 86_400), 0, &style));
    assert_eq!(at(0), "Сегодня 01:31");
    assert_eq!(at(1), "Вчера 01:31");
    assert_eq!(at(3), "Четверг 01:31");
    assert_eq!(at(30), "25 сен 01:31");
    assert_eq!(at(400), "20 сен 2025");
    assert_eq!(month_name(5), "Май");
    crate::i18n::set_lang(crate::i18n::Lang::En);
}
