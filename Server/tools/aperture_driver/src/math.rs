//! Aperture's rigid-body math. Quaternion layout is OpenXR xyzw; distances are metres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub position: [f64; 3],
    pub orientation: [f64; 4],
}
impl Default for Pose {
    fn default() -> Self {
        Self {
            position: [0.; 3],
            orientation: [0., 0., 0., 1.],
        }
    }
}
pub fn normalize(q: [f64; 4]) -> Option<[f64; 4]> {
    let n = q.iter().map(|v| v * v).sum::<f64>();
    if !n.is_finite() || !(0.25..=4.).contains(&n) {
        return None;
    }
    Some(q.map(|v| v / n.sqrt()))
}
pub fn multiply(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}
pub fn rotate(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
    let r = multiply(
        multiply(q, [v[0], v[1], v[2], 0.]),
        [-q[0], -q[1], -q[2], q[3]],
    );
    [r[0], r[1], r[2]]
}
pub fn compose(parent: Pose, child: Pose) -> Pose {
    let p = rotate(parent.orientation, child.position);
    Pose {
        position: std::array::from_fn(|i| parent.position[i] + p[i]),
        orientation: multiply(parent.orientation, child.orientation),
    }
}
pub fn inverse(p: Pose) -> Pose {
    let q = [
        -p.orientation[0],
        -p.orientation[1],
        -p.orientation[2],
        p.orientation[3],
    ];
    Pose {
        position: rotate(q, p.position.map(|v| -v)),
        orientation: q,
    }
}
pub fn finite_pose(p: Pose) -> Option<Pose> {
    if !p.position.iter().all(|v| v.is_finite()) {
        return None;
    }
    Some(Pose {
        orientation: normalize(p.orientation)?,
        ..p
    })
}
/// Real sample-to-sample motion, without smoothing or guessed extrapolation.
/// The quaternion's double cover is handled before taking the shortest rotation.
pub fn velocity(previous: Pose, current: Pose, seconds: f64) -> Option<([f64; 3], [f64; 3])> {
    if !(0.001..=0.1).contains(&seconds) {
        return None;
    }
    let previous = finite_pose(previous)?;
    let current = finite_pose(current)?;
    let mut delta = multiply(
        current.orientation,
        [
            -previous.orientation[0],
            -previous.orientation[1],
            -previous.orientation[2],
            previous.orientation[3],
        ],
    );
    if delta[3] < 0. {
        delta = delta.map(|v| -v);
    }
    let sin = (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt();
    let scale = if sin > 1e-9 {
        2. * sin.atan2(delta[3]) / sin / seconds
    } else {
        2. / seconds
    };
    let linear = std::array::from_fn(|i| (current.position[i] - previous.position[i]) / seconds);
    let angular = [delta[0] * scale, delta[1] * scale, delta[2] * scale];
    if linear.iter().any(|v| v.abs() > 20.) || angular.iter().any(|v| v.abs() > 40.) {
        return None;
    }
    Some((linear, angular))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reference_space_composition_is_not_commutative() {
        let angle = std::f64::consts::FRAC_PI_4;
        let parent = Pose {
            position: [2., 0., 0.],
            orientation: [0., 0., angle.sin(), angle.cos()],
        };
        let child = Pose {
            position: [1., 0., 0.],
            ..Pose::default()
        };
        let world = compose(parent, child);
        assert!((world.position[0] - 2.).abs() < 1e-10);
        assert!((world.position[1] - 1.).abs() < 1e-10);
        let local = compose(inverse(parent), world);
        for i in 0..3 {
            assert!((local.position[i] - child.position[i]).abs() < 1e-10);
        }
    }
    #[test]
    fn quaternion_sign_does_not_generate_motion() {
        let p = Pose::default();
        let q = Pose {
            orientation: [0., 0., 0., -1.],
            ..p
        };
        assert_eq!(velocity(p, q, 0.01).unwrap().1, [0.; 3]);
    }
    #[test]
    fn motion_uses_sample_interval_and_rejects_discontinuities() {
        let p = Pose::default();
        let q = Pose {
            position: [0.01, 0., 0.],
            ..p
        };
        assert_eq!(velocity(p, q, 0.01).unwrap().0, [1., 0., 0.]);
        assert!(velocity(p, q, 0.).is_none());
        assert!(velocity(
            p,
            Pose {
                position: [5., 0., 0.],
                ..p
            },
            0.01
        )
        .is_none());
        assert!(finite_pose(Pose {
            orientation: [0.; 4],
            ..p
        })
        .is_none());
    }
}
