//! Hard gates and circuit breakers evaluated before a tool call runs.
//!
//! Pure predicates over the turn state: they decide whether a call is denied
//! outright, without mutating anything.
