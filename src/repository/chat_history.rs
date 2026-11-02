use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;

static CHAT_EVENT_SENDER: LazyLock<broadcast::Sender<ChatTurnItem>> = LazyLock::new(|| {
    let (tx, _) = broadcast::channel(128);
    tx
});

pub fn get_chat_event_sender() -> broadcast::Sender<ChatTurnItem> {
    CHAT_EVENT_SENDER.clone()
}

#[cfg(windows)]
pub fn get_local_date_and_time() -> (String, String) {
    #[repr(C)]
    struct SystemTime {
        w_year: u16,
        w_month: u16,
        w_day_of_week: u16,
        w_day: u16,
        w_hour: u16,
        w_minute: u16,
        w_second: u16,
        w_milliseconds: u16,
    }
    #[repr(C)]
    struct TimeZoneInformation {
        bias: i32,
        standard_name: [u16; 32],
        standard_date: SystemTime,
        standard_bias: i32,
        daylight_name: [u16; 32],
        daylight_date: SystemTime,
        daylight_bias: i32,
    }

    unsafe extern "system" {
        fn GetLocalTime(lp_system_time: *mut SystemTime);
        fn GetTimeZoneInformation(lp_time_zone_information: *mut TimeZoneInformation) -> u32;
    }
    let mut st = SystemTime {
        w_year: 0,
        w_month: 0,
        w_day_of_week: 0,
        w_day: 0,
        w_hour: 0,
        w_minute: 0,
        w_second: 0,
        w_milliseconds: 0,
    };
    let mut tzi = std::mem::MaybeUninit::<TimeZoneInformation>::zeroed();

    unsafe {
        GetLocalTime(&mut st);
        GetTimeZoneInformation(tzi.as_mut_ptr());
    };

    let tzi = unsafe { tzi.assume_init() };
    let offset_mins = -tzi.bias;
    let sign = if offset_mins >= 0 { '+' } else { '-' };
    let abs_mins = offset_mins.abs();
    let off_hours = abs_mins / 60;
    let off_minutes = abs_mins % 60;

    (
        format!("{:04}-{:02}-{:02}", st.w_year, st.w_month, st.w_day),
        format!(
            "{:02}:{:02}:{:02}{}{:02}:{:02}",
            st.w_hour, st.w_minute, st.w_second, sign, off_hours, off_minutes
        ),
    )
}

#[cfg(not(windows))]
pub fn get_local_date_and_time() -> (String, String) {
    let now = std::time::SystemTime::now();
    let secs = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    #[repr(C)]
    struct Tm {
        tm_sec: i32,
        tm_min: i32,
        tm_hour: i32,
        tm_mday: i32,
        tm_mon: i32,
        tm_year: i32,
        tm_wday: i32,
        tm_yday: i32,
        tm_isdst: i32,
        tm_gmtoff: i64,
        tm_zone: *const std::os::raw::c_char,
    }

    unsafe extern "C" {
        fn localtime_r(time_p: *const i64, result: *mut Tm) -> *mut Tm;
    }

    let mut tm = Tm {
        tm_sec: 0,
        tm_min: 0,
        tm_hour: 0,
        tm_mday: 0,
        tm_mon: 0,
        tm_year: 0,
        tm_wday: 0,
        tm_yday: 0,
        tm_isdst: 0,
        tm_gmtoff: 0,
        tm_zone: std::ptr::null(),
    };

    unsafe {
        localtime_r(&secs, &mut tm);
    }

    let year = tm.tm_year + 1900;
    let month = tm.tm_mon + 1;
    let day = tm.tm_mday;
    let hour = tm.tm_hour;
    let minute = tm.tm_min;
    let second = tm.tm_sec;

    let off_secs = tm.tm_gmtoff;
    let sign = if off_secs >= 0 { '+' } else { '-' };
    let abs_secs = off_secs.abs();
    let off_hours = abs_secs / 3600;
    let off_minutes = (abs_secs % 3600) / 60;

    (
        format!("{:04}-{:02}-{:02}", year, month, day),
        format!(
            "{:02}:{:02}:{:02}{}{:02}:{:02}",
            hour, minute, second, sign, off_hours, off_minutes
        ),
    )
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ChatHistoryRecord {
    pub id: String,
    pub timestamp: i64,
    pub date_str: String,
    pub time_str: String,
    pub callsign: String,
    pub assistant_name: String,
    pub role: String,
    pub user_msg: String,
    pub assistant_msg: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatTurnItem {
    pub time: String,
    pub speaker: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChatHistoryPaginatedResponse {
    pub date: String,
    pub turns: Vec<ChatTurnItem>,
    pub total_turns: usize,
    pub page: usize,
    pub limit: usize,
    pub total_pages: usize,
    pub has_more: bool,
}

pub async fn save_chat_turn(
    pool: &SqlitePool,
    callsign: &str,
    assistant_name: &str,
    role: &str,
    user_msg: &str,
    assistant_msg: &str,
) -> Result<ChatHistoryRecord, sqlx::Error> {
    let (date_str, time_str) = get_local_date_and_time();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let id = format!("chat_{}_{}", timestamp, fastrand_hex(6));

    let record = ChatHistoryRecord {
        id,
        timestamp,
        date_str,
        time_str,
        callsign: callsign.to_string(),
        assistant_name: assistant_name.to_string(),
        role: role.to_string(),
        user_msg: user_msg.to_string(),
        assistant_msg: assistant_msg.to_string(),
    };

    sqlx::query(
        r#"
        INSERT INTO chat_history (
            id, timestamp, date_str, time_str, callsign, assistant_name, role, user_msg, assistant_msg
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
        "#,
    )
    .bind(&record.id)
    .bind(record.timestamp)
    .bind(&record.date_str)
    .bind(&record.time_str)
    .bind(&record.callsign)
    .bind(&record.assistant_name)
    .bind(&record.role)
    .bind(&record.user_msg)
    .bind(&record.assistant_msg)
    .execute(pool)
    .await?;

    Ok(record)
}

pub fn log_conversation_turn(
    callsign: &str,
    assistant_name: &str,
    role: &str,
    user_msg: &str,
    assistant_msg: &str,
) {
    let (date_str, time_str) = get_local_date_and_time();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let id = format!("chat_{}_{}", timestamp, fastrand_hex(6));

    let record = ChatHistoryRecord {
        id,
        timestamp,
        date_str: date_str.clone(),
        time_str: time_str.clone(),
        callsign: callsign.to_string(),
        assistant_name: assistant_name.to_string(),
        role: role.to_string(),
        user_msg: user_msg.to_string(),
        assistant_msg: assistant_msg.to_string(),
    };

    let rec = record;
    let user_turn = ChatTurnItem {
        time: rec.time_str.clone(),
        speaker: rec.callsign.clone(),
        message: rec.user_msg.clone(),
        id: Some(format!("{}-user", rec.id)),
        timestamp: Some(rec.timestamp),
    };
    let assistant_turn = ChatTurnItem {
        time: rec.time_str.clone(),
        speaker: rec.assistant_name.clone(),
        message: rec.assistant_msg.clone(),
        id: Some(format!("{}-asst", rec.id)),
        timestamp: Some(rec.timestamp),
    };
    let _ = CHAT_EVENT_SENDER.send(user_turn);
    let _ = CHAT_EVENT_SENDER.send(assistant_turn);

    tokio::spawn(async move {
        let pool = crate::llm::usage::get_pool().await;
        let _ = sqlx::query(
            r#"
            INSERT INTO chat_history (
                id, timestamp, date_str, time_str, callsign, assistant_name, role, user_msg, assistant_msg
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            "#,
        )
        .bind(&rec.id)
        .bind(rec.timestamp)
        .bind(&rec.date_str)
        .bind(&rec.time_str)
        .bind(&rec.callsign)
        .bind(&rec.assistant_name)
        .bind(&rec.role)
        .bind(&rec.user_msg)
        .bind(&rec.assistant_msg)
        .execute(pool)
        .await;
    });
    crate::term_log!("[History] Saved chat turn ({date_str} {time_str}) to SQLite");
}

pub async fn get_history_dates() -> Vec<String> {
    let pool = crate::llm::usage::get_pool().await;
    get_history_dates_from_pool(pool).await
}

pub async fn get_history_dates_from_pool(pool: &SqlitePool) -> Vec<String> {
    let mut dates: Vec<String> = Vec::new();

    if let Ok(rows) =
        sqlx::query_scalar::<_, String>("SELECT DISTINCT date_str FROM chat_history ORDER BY date_str DESC")
            .fetch_all(pool)
            .await
    {
        dates = rows;
    }

    dates
}

pub async fn get_history_by_date_paginated(
    target_date: &str,
    page: usize,
    limit: usize,
    order: Option<&str>,
) -> ChatHistoryPaginatedResponse {
    let pool = crate::llm::usage::get_pool().await;
    get_history_by_date_paginated_from_pool(pool, target_date, page, limit, order).await
}

pub async fn get_history_by_date_paginated_from_pool(
    pool: &SqlitePool,
    target_date: &str,
    page: usize,
    limit: usize,
    order: Option<&str>,
) -> ChatHistoryPaginatedResponse {
    let page = page.max(1);
    let limit = limit.clamp(1, 100);
    let offset = (page - 1) * limit;

    let total_rows: usize = sqlx::query("SELECT COUNT(*) as cnt FROM chat_history WHERE date_str = ?")
        .bind(target_date)
        .fetch_one(pool)
        .await
        .ok()
        .and_then(|row| {
            use sqlx::Row;
            row.try_get::<i64, _>("cnt").ok()
        })
        .map(|c| c.max(0) as usize)
        .unwrap_or(0);

    let total_turns = total_rows * 2;
    let total_pages = if total_rows == 0 {
        1
    } else {
        total_rows.div_ceil(limit)
    };

    let is_desc = order.map(|s| s.eq_ignore_ascii_case("desc")).unwrap_or(false);

    let rows = if is_desc {
        let mut r = sqlx::query_as::<_, ChatHistoryRecord>(
            "SELECT * FROM chat_history WHERE date_str = ? ORDER BY timestamp DESC, id DESC LIMIT ? OFFSET ?",
        )
        .bind(target_date)
        .bind(limit as i64)
        .bind(offset as i64)
        .fetch_all(pool)
        .await
        .unwrap_or_default();
        r.reverse();
        r
    } else {
        sqlx::query_as::<_, ChatHistoryRecord>(
            "SELECT * FROM chat_history WHERE date_str = ? ORDER BY timestamp ASC, id ASC LIMIT ? OFFSET ?",
        )
        .bind(target_date)
        .bind(limit as i64)
        .bind(offset as i64)
        .fetch_all(pool)
        .await
        .unwrap_or_default()
    };

    let mut turns = Vec::with_capacity(rows.len() * 2);
    for r in rows {
        turns.push(ChatTurnItem {
            time: r.time_str.clone(),
            speaker: r.callsign,
            message: r.user_msg,
            id: Some(format!("{}-user", r.id)),
            timestamp: Some(r.timestamp),
        });
        turns.push(ChatTurnItem {
            time: r.time_str,
            speaker: r.assistant_name,
            message: r.assistant_msg,
            id: Some(format!("{}-asst", r.id)),
            timestamp: Some(r.timestamp),
        });
    }

    let has_more = page < total_pages;

    ChatHistoryPaginatedResponse {
        date: target_date.to_string(),
        turns,
        total_turns,
        page,
        limit,
        total_pages,
        has_more,
    }
}

pub async fn delete_history_by_date(target_date: &str) -> anyhow::Result<bool> {
    let pool = crate::llm::usage::get_pool().await;
    delete_history_by_date_from_pool(pool, target_date).await
}

pub async fn delete_history_by_date_from_pool(
    pool: &SqlitePool,
    target_date: &str,
) -> anyhow::Result<bool> {
    let res = sqlx::query("DELETE FROM chat_history WHERE date_str = ?")
        .bind(target_date)
        .execute(pool)
        .await?;

    Ok(res.rows_affected() > 0)
}

fn fastrand_hex(len: usize) -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let s = RandomState::new().build_hasher().finish();
    let hex = format!("{:016x}", s);
    hex.chars().take(len).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    #[tokio::test]
    async fn test_chat_history_sqlite_operations() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        crate::repository::migrate_database(&pool).await.unwrap();

        let test_date = "2026-99-99";
        let rec_id = format!("test_{}", fastrand_hex(6));
        sqlx::query(
            r#"
            INSERT INTO chat_history (
                id, timestamp, date_str, time_str, callsign, assistant_name, role, user_msg, assistant_msg
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            "#,
        )
        .bind(&rec_id)
        .bind(123456789i64)
        .bind(test_date)
        .bind("12:00:00+00:00")
        .bind("User")
        .bind("Raga")
        .bind("user")
        .bind("hello")
        .bind("hi there")
        .execute(&pool)
        .await
        .unwrap();

        let dates = get_history_dates_from_pool(&pool).await;
        assert!(dates.contains(&test_date.to_string()));

        let paginated = get_history_by_date_paginated_from_pool(&pool, test_date, 1, 10, None).await;
        assert_eq!(paginated.total_turns, 2);
        assert_eq!(paginated.turns.len(), 2);
        assert_eq!(paginated.turns[0].message, "hello");
        assert_eq!(paginated.turns[1].message, "hi there");

        let deleted = delete_history_by_date_from_pool(&pool, test_date).await.unwrap();
        assert!(deleted);

        let dates_after = get_history_dates_from_pool(&pool).await;
        assert!(!dates_after.contains(&test_date.to_string()));
    }
}
