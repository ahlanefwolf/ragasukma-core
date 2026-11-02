pub const SCORE_MODIFIED: f64 = 1.0;
pub const SCORE_READ: f64 = 0.4;
pub const RECENCY_HALF_LIFE_SECS: f64 = 3_600.0;
pub const RECENCY_FLOOR_SHARE: f64 = 0.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceOrigin {
    Read,
    Modified,
}

pub fn base_score(origin: EvidenceOrigin) -> f64 {
    match origin {
        EvidenceOrigin::Modified => SCORE_MODIFIED,
        EvidenceOrigin::Read => SCORE_READ,
    }
}

pub fn recency_factor(age_secs: i64) -> f64 {
    if age_secs <= 0 {
        return 1.0;
    }
    let age = age_secs as f64;
    let decayed = 1.0 - (1.0 - RECENCY_FLOOR_SHARE) * (age / RECENCY_HALF_LIFE_SECS).min(1.0);
    decayed.max(RECENCY_FLOOR_SHARE)
}

pub fn rank_evidence(origin: EvidenceOrigin, age_secs: i64) -> f64 {
    base_score(origin) * recency_factor(age_secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modified_files_outrank_read_files_at_equal_age() {
        assert!(rank_evidence(EvidenceOrigin::Modified, 0) > rank_evidence(EvidenceOrigin::Read, 0));
        assert!(
            rank_evidence(EvidenceOrigin::Modified, i64::MAX / 2)
                > rank_evidence(EvidenceOrigin::Read, i64::MAX / 2),
            "an edited file must stay ahead of a read file however old both are"
        );
    }

    #[test]
    fn recent_entries_outrank_old_entries_of_the_same_origin() {
        for origin in [EvidenceOrigin::Read, EvidenceOrigin::Modified] {
            assert!(rank_evidence(origin, 0) > rank_evidence(origin, 600));
            assert!(rank_evidence(origin, 600) > rank_evidence(origin, 10_000));
        }
    }

    #[test]
    fn recency_never_evicts_old_context_completely() {
        let ancient_edit = rank_evidence(EvidenceOrigin::Modified, i64::MAX / 2);
        let fresh_read = rank_evidence(EvidenceOrigin::Read, 0);
        assert!(
            ancient_edit > fresh_read,
            "ancient edit {ancient_edit} must still beat fresh read {fresh_read}"
        );
        assert!(ancient_edit > 0.0, "floor must keep old evidence non-zero");
    }

    #[test]
    fn unknown_age_does_not_demote() {
        for origin in [EvidenceOrigin::Read, EvidenceOrigin::Modified] {
            assert_eq!(rank_evidence(origin, 0), base_score(origin));
        }
        assert_eq!(rank_evidence(EvidenceOrigin::Modified, -500), SCORE_MODIFIED);
    }

    #[test]
    fn scores_stay_within_the_shared_range() {
        for origin in [EvidenceOrigin::Read, EvidenceOrigin::Modified] {
            for age in [0i64, 1, 600, 3_600, 86_400, i64::MAX / 2] {
                let score = rank_evidence(origin, age);
                assert!((0.0..=1.0).contains(&score), "{origin:?}/{age} produced {score}");
                assert!(!score.is_nan(), "{origin:?}/{age} must never yield NaN");
            }
        }
    }

    #[test]
    fn ordering_is_total_and_stable() {
        let mut scores: Vec<f64> = [EvidenceOrigin::Read, EvidenceOrigin::Modified]
            .into_iter()
            .flat_map(|o| [0i64, 600, 86_400].map(move |a| rank_evidence(o, a)))
            .collect();
        scores.sort_by(|a, b| b.total_cmp(a));
        assert_eq!(scores.first().copied(), Some(SCORE_MODIFIED));
        assert!(scores.iter().all(|s| s.is_finite()));
    }
}