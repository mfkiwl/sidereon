use std::collections::BTreeSet;

use sidereon_core::data::{
    distribution_location_for_identity, product, AnalysisCenter, ArchiveCompression,
    DistributionSource, ProductDate, ProductType,
};
use sidereon_core::ephemeris::{
    MergeCombine, MergeOptions, Sp3ArtifactIdentity, Sp3FrameLabelSet,
    Sp3FrameReconciliationOptions, Sp3MergeInputIdentity, Sp3MergeInputIdentityError,
};
use sidereon_core::GnssSystem;

fn identity(center: AnalysisCenter) -> sidereon_core::data::ProductIdentity {
    let issue = match center {
        AnalysisCenter::IgsUlt
        | AnalysisCenter::CodUlt
        | AnalysisCenter::EsaUlt
        | AnalysisCenter::GfzUlt => Some("0000"),
        _ => None,
    };
    product(
        center,
        ProductType::Sp3,
        ProductDate::new(2026, 7, 16).unwrap(),
        None,
        issue,
    )
    .unwrap()
    .identity()
    .unwrap()
}

fn artifact(center: AnalysisCenter, byte: u8) -> Sp3ArtifactIdentity {
    let requested_identity = identity(center);
    let mut resolved_identity = requested_identity.clone();
    resolved_identity.format_version = Some("d".to_string());
    Sp3ArtifactIdentity {
        official_filename: requested_identity.official_filename.clone(),
        requested_identity,
        resolved_identity,
        distribution_source: DistributionSource::Direct,
        product_sha256: format!("{byte:02x}").repeat(32),
        product_byte_length: 12_345,
        archive_sha256: format!("{:02x}", byte.wrapping_add(1)).repeat(32),
        archive_byte_length: 6_789,
        compression: ArchiveCompression::Gzip,
    }
}

#[test]
fn merge_input_identity_is_order_independent_and_verifiable() {
    let first = artifact(AnalysisCenter::Esa, 0x11);
    let second = artifact(AnalysisCenter::Cod, 0x22);
    let policy = MergeOptions::default();

    let forward = Sp3MergeInputIdentity::new(&[first.clone(), second.clone()], &policy).unwrap();
    let reverse = Sp3MergeInputIdentity::new(&[second.clone(), first.clone()], &policy).unwrap();

    assert_eq!(forward.stable_id, reverse.stable_id);
    assert_eq!(forward.contributors, reverse.contributors);
    assert!(forward.verify(&policy).unwrap());
    assert!(forward
        .verify_against(&[second.clone(), first.clone()], &policy)
        .unwrap());

    let mut corrupted_record = forward.clone();
    corrupted_record.contributors[0].product_sha256 = "ff".repeat(32);
    assert!(!corrupted_record.verify(&policy).unwrap());
    assert!(!corrupted_record
        .verify_against(&[second, first], &policy)
        .unwrap());
}

#[test]
fn precedence_order_is_bound_as_semantic_merge_policy() {
    let first = artifact(AnalysisCenter::Esa, 0x11);
    let second = artifact(AnalysisCenter::Cod, 0x22);
    let policy = MergeOptions {
        combine: MergeCombine::Precedence,
        ..MergeOptions::default()
    };

    let forward = Sp3MergeInputIdentity::new(&[first.clone(), second.clone()], &policy).unwrap();
    let reverse = Sp3MergeInputIdentity::new(&[second, first], &policy).unwrap();

    assert_ne!(forward.stable_id, reverse.stable_id);
    assert_eq!(forward.contributors, reverse.contributors);
    assert_ne!(
        forward.precedence_contributors,
        reverse.precedence_contributors
    );
    assert!(forward.verify(&policy).unwrap());
    assert!(reverse.verify(&policy).unwrap());
}

#[test]
fn artifact_or_policy_changes_change_the_stable_identity() {
    let first = artifact(AnalysisCenter::Esa, 0x11);
    let second = artifact(AnalysisCenter::Cod, 0x22);
    let original =
        Sp3MergeInputIdentity::new(&[first.clone(), second.clone()], &MergeOptions::default())
            .unwrap();

    let mut changed_artifact = second.clone();
    changed_artifact.product_sha256 = "33".repeat(32);
    let changed_artifact =
        Sp3MergeInputIdentity::new(&[first.clone(), changed_artifact], &MergeOptions::default())
            .unwrap();
    assert_ne!(original.stable_id, changed_artifact.stable_id);

    let changed_policy = MergeOptions {
        combine: MergeCombine::Median,
        ..MergeOptions::default()
    };
    let changed_policy = Sp3MergeInputIdentity::new(&[first, second], &changed_policy).unwrap();
    assert_ne!(original.stable_id, changed_policy.stable_id);
}

#[test]
fn policy_set_order_does_not_change_the_stable_identity() {
    let contributor = artifact(AnalysisCenter::Esa, 0x11);
    let first = MergeOptions {
        systems: Some(BTreeSet::from([GnssSystem::Galileo, GnssSystem::Gps])),
        frame_reconciliation: Sp3FrameReconciliationOptions {
            asserted_equivalent_label_sets: vec![
                Sp3FrameLabelSet::new(["IGS20", "ITRF2020"]),
                Sp3FrameLabelSet::new(["IGS14", "ITRF2014"]),
            ],
            helmert: false,
        },
        ..MergeOptions::default()
    };

    let second = MergeOptions {
        systems: Some(BTreeSet::from([GnssSystem::Gps, GnssSystem::Galileo])),
        frame_reconciliation: Sp3FrameReconciliationOptions {
            asserted_equivalent_label_sets: vec![
                Sp3FrameLabelSet::new(["ITRF2014", "IGS14"]),
                Sp3FrameLabelSet::new(["ITRF2020", "IGS20"]),
            ],
            helmert: false,
        },
        ..MergeOptions::default()
    };

    let first = Sp3MergeInputIdentity::new(std::slice::from_ref(&contributor), &first).unwrap();
    let second = Sp3MergeInputIdentity::new(&[contributor], &second).unwrap();
    assert_eq!(first.stable_id, second.stable_id);
}

#[test]
fn incomplete_or_mismatched_provenance_fails_closed() {
    let mut contributor = artifact(AnalysisCenter::Esa, 0x11);
    contributor.archive_sha256 = "not-a-digest".to_string();
    assert!(matches!(
        Sp3MergeInputIdentity::new(&[contributor], &MergeOptions::default()),
        Err(Sp3MergeInputIdentityError::InvalidContributor { .. })
    ));

    let mut contributor = artifact(AnalysisCenter::Esa, 0x11);
    contributor.resolved_identity = identity(AnalysisCenter::Cod);
    assert!(matches!(
        Sp3MergeInputIdentity::new(&[contributor], &MergeOptions::default()),
        Err(Sp3MergeInputIdentityError::InvalidContributor { .. })
    ));

    let mut contributor = artifact(AnalysisCenter::Esa, 0x11);
    contributor.resolved_identity.format_version = None;
    assert!(matches!(
        Sp3MergeInputIdentity::new(&[contributor], &MergeOptions::default()),
        Err(Sp3MergeInputIdentityError::InvalidContributor { .. })
    ));
}

#[test]
fn direct_location_retains_alternate_exact_filename() {
    let mut alternate = identity(AnalysisCenter::CodUlt);
    alternate.span = "02D".to_string();
    alternate.official_filename = alternate.official_filename.replace("_01D_", "_02D_");
    alternate.validate().unwrap();

    let location =
        distribution_location_for_identity(&alternate, DistributionSource::Direct).unwrap();
    assert!(location
        .original_url
        .as_deref()
        .unwrap()
        .ends_with(&location.archive_filename));
    assert!(location
        .archive_filename
        .starts_with(&alternate.official_filename));
}
