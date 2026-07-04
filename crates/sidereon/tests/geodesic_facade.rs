use sidereon::{geodesic_direct, geodesic_inverse};

#[test]
fn facade_reexports_geodesic_helpers() {
    let core_inverse = sidereon_core::geodesic_inverse(
        40.64,
        -73.78,
        32.621_100_463_725_796,
        49.052_487_092_959_82,
    )
    .expect("core inverse");
    let facade_inverse =
        geodesic_inverse(40.64, -73.78, 32.621_100_463_725_796, 49.052_487_092_959_82)
            .expect("facade inverse");
    assert_eq!(facade_inverse, core_inverse);

    let core_direct =
        sidereon_core::geodesic_direct(40.64, -73.78, 45.0, 10_000_000.0).expect("core direct");
    let facade_direct = geodesic_direct(40.64, -73.78, 45.0, 10_000_000.0).expect("facade direct");
    assert_eq!(facade_direct, core_direct);
}
