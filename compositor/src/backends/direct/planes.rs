use smithay::backend::drm::compositor::FrameFlags;

pub(super) fn frame_flags(fullscreen: bool, animating: bool, locked: bool) -> FrameFlags {
    let mut flags = FrameFlags::ALLOW_CURSOR_PLANE_SCANOUT;

    if fullscreen && !animating && !locked {
        flags |= FrameFlags::ALLOW_PRIMARY_PLANE_SCANOUT;
    }

    // Overlay planes stay disabled until their effects and capture behavior
    // have been validated on hardware independently of fullscreen scanout.
    flags
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_settled_unlocked_fullscreen_frames_allow_primary_scanout() {
        for fullscreen in [false, true] {
            for animating in [false, true] {
                for locked in [false, true] {
                    let flags = frame_flags(fullscreen, animating, locked);
                    assert_eq!(
                        flags.contains(FrameFlags::ALLOW_PRIMARY_PLANE_SCANOUT),
                        fullscreen && !animating && !locked,
                    );
                    assert!(flags.contains(FrameFlags::ALLOW_CURSOR_PLANE_SCANOUT));
                    assert!(!flags.intersects(
                        FrameFlags::ALLOW_PRIMARY_PLANE_SCANOUT_ANY
                            | FrameFlags::ALLOW_OVERLAY_PLANE_SCANOUT
                            | FrameFlags::SKIP_CURSOR_ONLY_UPDATES
                    ));
                }
            }
        }
    }
}
