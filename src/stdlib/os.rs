use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::{Callback, CallbackReturn, Context, FromValue, String, Table, Value};

/// Load the `os` library: clock, date, difftime, exit, getenv, remove, rename, time, tmpname.
///
/// Time handling is implemented in pure Rust (no libc): `os.date`/`os.time` interpret broken-down
/// times as UTC. Local-time zone offsets are not applied (documented limitation consistent with
/// piccolo's "C locale / fixed environment" emulation aim).
pub fn load_os<'gc>(ctx: Context<'gc>) {
    let os = Table::new(&ctx);

    os.set_field(
        ctx,
        "clock",
        Callback::from_fn(&ctx, |ctx, _, mut stack| {
            static START: OnceLock<Instant> = OnceLock::new();
            let start = START.get_or_init(Instant::now);
            let elapsed = start.elapsed().as_secs_f64();
            stack.replace(ctx, elapsed);
            Ok(CallbackReturn::Return)
        }),
    );

    os.set_field(
        ctx,
        "difftime",
        Callback::from_fn(&ctx, |ctx, _, mut stack| {
            let (t2, t1) = stack.consume::<(f64, f64)>(ctx)?;
            stack.replace(ctx, t2 - t1);
            Ok(CallbackReturn::Return)
        }),
    );

    os.set_field(
        ctx,
        "time",
        Callback::from_fn(&ctx, |ctx, _, mut stack| {
            let arg = stack.get(0);
            if arg.is_nil() {
                let secs = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                stack.replace(ctx, secs);
            } else {
                let table = Table::from_value(ctx, arg)?;
                let year = get_int_field(ctx, table, "year")?.unwrap_or(1900);
                let month = get_int_field(ctx, table, "month")?.unwrap_or(1);
                let day = get_int_field(ctx, table, "day")?.unwrap_or(1);
                let hour = get_int_field(ctx, table, "hour")?.unwrap_or(12);
                let min = get_int_field(ctx, table, "min")?.unwrap_or(0);
                let sec = get_int_field(ctx, table, "sec")?.unwrap_or(0);
                let days = days_from_civil(year, month, day);
                let secs = days * 86400 + hour * 3600 + min * 60 + sec;
                stack.replace(ctx, secs);
            }
            Ok(CallbackReturn::Return)
        }),
    );

    os.set_field(
        ctx,
        "date",
        Callback::from_fn(&ctx, |ctx, _, mut stack| {
            let (format, time) = stack.consume::<(Option<String>, Option<i64>)>(ctx)?;
            let format = format.unwrap_or_else(|| ctx.intern(b"%c"));
            let time = time.unwrap_or_else(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0)
            });

            let fmt = format.as_bytes();
            // A leading '!' selects UTC (which is all we support); strip it.
            let fmt = if fmt.first() == Some(&b'!') { &fmt[1..] } else { fmt };

            let broken = BrokenTime::from_secs(time);
            let out = format_date(fmt, &broken);
            stack.replace(ctx, ctx.intern(out.as_bytes()));
            Ok(CallbackReturn::Return)
        }),
    );

    os.set_field(
        ctx,
        "getenv",
        Callback::from_fn(&ctx, |ctx, _, mut stack| {
            let name = stack.consume::<String>(ctx)?;
            let key = name.display_lossy().to_string();
            match std::env::var(key) {
                Ok(val) => stack.replace(ctx, ctx.intern(val.as_bytes())),
                Err(_) => stack.replace(ctx, Value::Nil),
            }
            Ok(CallbackReturn::Return)
        }),
    );

    os.set_field(
        ctx,
        "remove",
        Callback::from_fn(&ctx, |ctx, _, mut stack| {
            let name = stack.consume::<String>(ctx)?;
            let path = name.display_lossy().to_string();
            match std::fs::remove_file(&path) {
                Ok(()) => stack.replace(ctx, true),
                Err(e) => {
                    stack.replace(ctx, (Value::Nil, ctx.intern(e.to_string().as_bytes())));
                }
            }
            Ok(CallbackReturn::Return)
        }),
    );

    os.set_field(
        ctx,
        "rename",
        Callback::from_fn(&ctx, |ctx, _, mut stack| {
            let (from, to) = stack.consume::<(String, String)>(ctx)?;
            let from = from.display_lossy().to_string();
            let to = to.display_lossy().to_string();
            match std::fs::rename(&from, &to) {
                Ok(()) => stack.replace(ctx, true),
                Err(e) => {
                    stack.replace(ctx, (Value::Nil, ctx.intern(e.to_string().as_bytes())));
                }
            }
            Ok(CallbackReturn::Return)
        }),
    );

    os.set_field(
        ctx,
        "tmpname",
        Callback::from_fn(&ctx, |ctx, _, mut stack| {
            let mut dir = std::env::temp_dir();
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let name = format!("piccolo_tmp_{nanos:x}");
            dir.push(name);
            let path = dir.to_string_lossy().to_string();
            stack.replace(ctx, ctx.intern(path.as_bytes()));
            Ok(CallbackReturn::Return)
        }),
    );

    os.set_field(
        ctx,
        "exit",
        Callback::from_fn(&ctx, |_ctx, _, stack| {
            let code = stack.get(0);
            let exit_code = match code {
                Value::Boolean(b) => {
                    if b {
                        0
                    } else {
                        1
                    }
                }
                Value::Integer(i) => i as i32,
                Value::Number(n) => n as i32,
                _ => 0,
            };
            std::process::exit(exit_code);
        }),
    );

    ctx.set_global("os", os);
}

fn get_int_field<'gc>(
    ctx: Context<'gc>,
    table: Table<'gc>,
    field: &'static str,
) -> Result<Option<i64>, crate::Error<'gc>> {
    let v = table.get_value(ctx, field);
    if v.is_nil() {
        Ok(None)
    } else {
        Ok(Some(i64::from_value(ctx, v)?))
    }
}

// ---- Civil calendar conversion (Howard Hinnant's algorithms), UTC ----

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as i64; // [0, 399]
    let mp = if m > 2 { m - 3 } else { m + 9 }; // [0, 11]
    let doy = (153 * mp + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

struct BrokenTime {
    year: i64,
    month: i64, // 1-12
    day: i64,   // 1-31
    hour: i64,
    min: i64,
    sec: i64,
    weekday: i64, // 0 = Sunday
    yearday: i64, // 1-based
}

impl BrokenTime {
    fn from_secs(secs: i64) -> Self {
        let days = secs.div_euclid(86400);
        let rem = secs.rem_euclid(86400);
        let hour = rem / 3600;
        let min = (rem % 3600) / 60;
        let sec = rem % 60;
        let (year, month, day) = civil_from_days(days);
        // 1970-01-01 was a Thursday (weekday 4).
        let weekday = ((days + 4).rem_euclid(7)) as i64;
        let jan1 = days_from_civil(year, 1, 1);
        let yearday = (days - jan1 + 1) as i64;
        BrokenTime {
            year,
            month,
            day,
            hour,
            min,
            sec,
            weekday,
            yearday,
        }
    }
}

const WD_ABBR: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const WD_FULL: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const MON_ABBR: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const MON_FULL: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

fn format_date(fmt: &[u8], t: &BrokenTime) -> std::string::String {
    let mut out = std::string::String::new();
    let mut i = 0;
    while i < fmt.len() {
        let c = fmt[i];
        if c != b'%' || i + 1 >= fmt.len() {
            out.push(c as char);
            i += 1;
            continue;
        }
        let spec = fmt[i + 1];
        i += 2;
        match spec {
            b'Y' => out.push_str(&format!("{:04}", t.year)),
            b'y' => out.push_str(&format!("{:02}", t.year.rem_euclid(100))),
            b'm' => out.push_str(&format!("{:02}", t.month)),
            b'd' => out.push_str(&format!("{:02}", t.day)),
            b'H' => out.push_str(&format!("{:02}", t.hour)),
            b'I' => {
                let h = t.hour % 12;
                out.push_str(&format!("{:02}", if h == 0 { 12 } else { h }));
            }
            b'M' => out.push_str(&format!("{:02}", t.min)),
            b'S' => out.push_str(&format!("{:02}", t.sec)),
            b'p' => out.push_str(if t.hour < 12 { "AM" } else { "PM" }),
            b'a' => out.push_str(WD_ABBR[t.weekday as usize]),
            b'A' => out.push_str(WD_FULL[t.weekday as usize]),
            b'b' | b'h' => out.push_str(MON_ABBR[(t.month - 1) as usize]),
            b'B' => out.push_str(MON_FULL[(t.month - 1) as usize]),
            b'j' => out.push_str(&format!("{:03}", t.yearday)),
            b'w' => out.push_str(&format!("{}", t.weekday)),
            b'%' => out.push('%'),
            b'n' => out.push('\n'),
            b't' => out.push('\t'),
            b'c' => {
                // %a %b %e %H:%M:%S %Y
                out.push_str(WD_ABBR[t.weekday as usize]);
                out.push(' ');
                out.push_str(MON_ABBR[(t.month - 1) as usize]);
                out.push_str(&format!(" {:2} ", t.day));
                out.push_str(&format!("{:02}:{:02}:{:02} ", t.hour, t.min, t.sec));
                out.push_str(&format!("{:04}", t.year));
            }
            b'x' => out.push_str(&format!("{:02}/{:02}/{:02}", t.month, t.day, t.year.rem_euclid(100))),
            b'X' => out.push_str(&format!("{:02}:{:02}:{:02}", t.hour, t.min, t.sec)),
            b'R' => out.push_str(&format!("{:02}:{:02}", t.hour, t.min)),
            b'F' => out.push_str(&format!("{:04}-{:02}-{:02}", t.year, t.month, t.day)),
            b'T' => out.push_str(&format!("{:02}:{:02}:{:02}", t.hour, t.min, t.sec)),
            other => {
                // Unknown specifier: emit literally.
                out.push('%');
                out.push(other as char);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_roundtrip() {
        // 2000-01-01 is day 10957 from epoch.
        let days = days_from_civil(2000, 1, 1);
        assert_eq!(days, 10957);
        assert_eq!(civil_from_days(10957), (2000, 1, 1));
        // Epoch.
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }

    #[test]
    fn broken_time_epoch() {
        let t = BrokenTime::from_secs(0);
        assert_eq!(t.year, 1970);
        assert_eq!(t.month, 1);
        assert_eq!(t.day, 1);
        assert_eq!(t.hour, 0);
        assert_eq!(t.weekday, 4); // Thursday
    }

    #[test]
    fn date_format() {
        // 2021-06-15 12:30:45 UTC = 1623760245
        let t = BrokenTime::from_secs(1623760245);
        assert_eq!(format_date(b"%Y-%m-%d", &t), "2021-06-15");
        assert_eq!(format_date(b"%H:%M:%S", &t), "12:30:45");
        assert_eq!(format_date(b"%%", &t), "%");
    }
}
