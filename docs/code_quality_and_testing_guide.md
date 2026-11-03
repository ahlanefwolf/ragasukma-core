```bash
cargo clippy --lib -- \
  -W dead_code \
  -W unused \
  -W clippy::redundant_clone \
  -W clippy::await_holding_lock \
  -W clippy::mutex_atomic \
  -W clippy::rc_buffer \
  -W clippy::panic \
  -W clippy::unwrap_used
```
