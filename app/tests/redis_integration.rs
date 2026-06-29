use std::time::{Duration, SystemTime, UNIX_EPOCH};

use kgproxy::cache::{CachedEntry, RedisCache, ResponseCache};
use serde_json::json;

#[tokio::test]
async fn redis_cache_round_trips_cached_entry_when_redis_url_is_set() {
    let Ok(redis_url) = std::env::var("REDIS_URL") else {
        eprintln!("skipping redis integration test because REDIS_URL is not set");
        return;
    };

    let cache = RedisCache::new(&redis_url).expect("redis url should be valid");
    let key = format!(
        "kgproxy:test:{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let entry = CachedEntry::fresh(json!({ "id": "Albert_Einstein" }));

    cache
        .set(&key, &entry, Duration::from_secs(30))
        .await
        .expect("redis set should succeed");

    let stored = cache
        .get(&key)
        .await
        .expect("redis get should succeed")
        .expect("entry should exist");

    assert_eq!(stored.payload["id"], "Albert_Einstein");
    assert_eq!(stored.hit_count, 0);
}
