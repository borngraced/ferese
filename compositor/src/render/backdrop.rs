use super::*;

#[derive(Debug, Default)]
pub(super) struct Backdrop {
    pub commit: CommitCounter,
    sources: Vec<Source>,
    region: Option<Rectangle<i32, Physical>>,
}

#[derive(Debug)]
struct Source {
    id: Id,
    commit: CommitCounter,
    geometry: Rectangle<i32, Physical>,
    alpha: f32,
    src: Rectangle<f64, Buffer>,
    transform: Transform,
    visible: Vec<Rectangle<i32, Physical>>,
}

impl Backdrop {
    fn update<E: Element>(
        &mut self,
        elements: &[E],
        region: Rectangle<i32, Physical>,
        scale: RenderScale<f64>,
    ) -> bool {
        let mut uncovered = vec![region];
        let mut sources = Vec::new();
        let mut changed = self.region != Some(region);

        for element in elements {
            let geometry = element.geometry(scale);
            let visible = uncovered
                .iter()
                .filter_map(|rect| rect.intersection(geometry))
                .collect::<Vec<_>>();
            if visible.is_empty() || element.alpha() == 0.0 {
                continue;
            }

            let previous = self.sources.iter().find(|source| &source.id == element.id());
            changed |= previous.is_none_or(|source| {
                source.geometry != geometry
                    || source.alpha != element.alpha()
                    || source.src != element.src()
                    || source.transform != element.transform()
                    || source.visible != visible
            });
            if let Some(previous) = previous {
                changed |= element.damage_since(scale, Some(previous.commit)).iter().any(|damage| {
                    let damage = Rectangle::new(damage.loc + geometry.loc, damage.size);
                    visible.iter().any(|rect| rect.overlaps(damage))
                });
            }

            sources.push(Source {
                id: element.id().clone(),
                commit: element.current_commit(),
                geometry,
                alpha: element.alpha(),
                src: element.src(),
                transform: element.transform(),
                visible,
            });
            if element.alpha() == 1.0 {
                for opaque in element.opaque_regions(scale).iter() {
                    let opaque = Rectangle::new(opaque.loc + geometry.loc, opaque.size);
                    uncovered = uncovered
                        .into_iter()
                        .flat_map(|rect| rect.subtract_rect(opaque))
                        .collect();
                }
            }
        }

        // Membership and stacking changes matter even without buffer damage.
        changed |= !self
            .sources
            .iter()
            .map(|source| &source.id)
            .eq(sources.iter().map(|source| &source.id));
        self.sources = sources;
        self.region = Some(region);
        if changed {
            self.commit.increment();
        }

        changed
    }
}

pub(super) fn update(elements: &[AnimatedWindowRenderElement], scale: f64) {
    // Resolve lower blurs first so their damage propagates to higher materials.
    for (index, element) in elements.iter().enumerate().rev() {
        if let AnimatedWindowRenderElement::Blur(blur) = element {
            let changed =
                blur.backdrop
                    .lock()
                    .unwrap()
                    .update(&elements[index + 1..], blur.geometry(scale.into()), scale.into());
            if changed {
                blur.capture_dirty.store(true, Ordering::Relaxed);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestElement {
        id: Id,
        commit: CommitCounter,
        rect: Rectangle<i32, Physical>,
        damage: Vec<Rectangle<i32, Physical>>,
        opaque: bool,
        alpha: f32,
    }

    impl TestElement {
        fn new(rect: Rectangle<i32, Physical>) -> Self {
            Self {
                id: Id::new(),
                commit: CommitCounter::default(),
                rect,
                damage: vec![],
                opaque: false,
                alpha: 1.0,
            }
        }

        fn repaint(&mut self, damage: Vec<Rectangle<i32, Physical>>) {
            self.commit.increment();
            self.damage = damage;
        }
    }

    impl Element for TestElement {
        fn id(&self) -> &Id {
            &self.id
        }
        fn current_commit(&self) -> CommitCounter {
            self.commit
        }
        fn src(&self) -> Rectangle<f64, Buffer> {
            Rectangle::from_size((100.0, 100.0).into())
        }
        fn geometry(&self, _: RenderScale<f64>) -> Rectangle<i32, Physical> {
            self.rect
        }
        fn damage_since(&self, _: RenderScale<f64>, commit: Option<CommitCounter>) -> DamageSet<i32, Physical> {
            if commit == Some(self.commit) {
                DamageSet::default()
            } else {
                DamageSet::from_slice(&self.damage)
            }
        }
        fn opaque_regions(&self, _: RenderScale<f64>) -> OpaqueRegions<i32, Physical> {
            if self.opaque {
                OpaqueRegions::from_slice(&[Rectangle::from_size(self.rect.size)])
            } else {
                OpaqueRegions::default()
            }
        }
        fn alpha(&self) -> f32 {
            self.alpha
        }
    }

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Physical> {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    #[test]
    fn only_damage_intersecting_the_sample_invalidates_its_output() {
        let mut first = Backdrop::default();
        let mut second = Backdrop::default();
        let region = rect(20, 20, 30, 30);
        let mut a = TestElement::new(rect(0, 0, 100, 100));
        let b = TestElement::new(rect(0, 0, 100, 100));
        assert!(first.update(&[&a], region, 1.0.into()));
        assert!(second.update(&[&b], region, 1.0.into()));

        a.repaint(vec![]);
        assert!(!first.update(&[&a], region, 1.0.into()));
        a.repaint(vec![rect(60, 60, 10, 10)]);
        assert!(!first.update(&[&a], region, 1.0.into()));
        a.repaint(vec![rect(25, 25, 1, 1)]);
        assert!(first.update(&[&a], region, 1.0.into()));
        assert!(!second.update(&[&b], region, 1.0.into()));
        assert!(!first.update(&[&a], region, 1.0.into()));
    }

    #[test]
    fn opaque_occlusion_hides_damage_but_fading_and_moving_reveal_it() {
        let mut backdrop = Backdrop::default();
        let region = rect(0, 0, 100, 100);
        let mut front = TestElement::new(rect(0, 0, 50, 100));
        front.opaque = true;
        let mut back = TestElement::new(region);
        assert!(backdrop.update(&[&front, &back], region, 1.0.into()));

        back.repaint(vec![rect(10, 10, 10, 10)]);
        assert!(!backdrop.update(&[&front, &back], region, 1.0.into()));
        front.alpha = 0.5;
        assert!(backdrop.update(&[&front, &back], region, 1.0.into()));
        back.repaint(vec![rect(10, 10, 10, 10)]);
        assert!(backdrop.update(&[&front, &back], region, 1.0.into()));
        front.rect.loc.x = 20;
        assert!(backdrop.update(&[&front, &back], region, 1.0.into()));
        assert!(!backdrop.update(&[&front, &back], region, 1.0.into()));
    }

    #[test]
    fn membership_and_stacking_changes_invalidate_without_buffer_damage() {
        let mut backdrop = Backdrop::default();
        let region = rect(0, 0, 100, 100);
        let a = TestElement::new(region);
        let b = TestElement::new(region);
        assert!(backdrop.update(&[&a, &b], region, 1.0.into()));
        assert!(backdrop.update(&[&b, &a], region, 1.0.into()));
        assert!(backdrop.update(&[&a], region, 1.0.into()));
        assert!(backdrop.update(&[&a, &b], region, 1.0.into()));
        assert!(!backdrop.update(&[&a, &b], region, 1.0.into()));
    }
}
