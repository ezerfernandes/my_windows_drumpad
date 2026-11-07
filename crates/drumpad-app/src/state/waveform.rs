//! Waveform data and viewport math.
//!
//! [`PeakSet`] is a multi-resolution min/max envelope. Level 0 has one bucket
//! per [`PeakSet::base_bucket`] frames; every further level halves the bucket
//! count, so a 10-minute sample can be drawn at any zoom without rescanning the
//! audio. The samples crate owns the durable peak cache (E04-T03); this type is
//! the UI's in-memory envelope and its shape is deliberately small enough to be
//! swapped for that cache later.
//!
//! [`WaveformView`] is pure viewport math: zoom, scroll, selection, playhead.
//! It has no egui dependency, so the hit-testing rules that make the editor feel
//! right are unit-tested without a window.

/// Smallest bucket size at the finest peak level.
pub const BASE_BUCKET_FRAMES: usize = 64;

/// Highest zoom: one source second can fill 1024 screens.
pub const MAX_ZOOM: f32 = 1024.0;

/// One min/max bucket.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Peak {
    /// Minimum sample value in the bucket.
    pub min: f32,
    /// Maximum sample value in the bucket.
    pub max: f32,
}

/// One resolution level.
#[derive(Debug, Clone, PartialEq)]
pub struct PeakLevel {
    /// Frames covered by one bucket.
    pub frames_per_bucket: usize,
    /// Buckets in source order.
    pub buckets: Vec<Peak>,
}

/// Multi-resolution envelopes for one sample.
#[derive(Debug, Clone, PartialEq)]
pub struct PeakSet {
    /// Frames per channel in the source.
    pub frames: usize,
    /// Frames per bucket at level 0.
    pub base_bucket: usize,
    /// Levels from finest to coarsest.
    pub levels: Vec<PeakLevel>,
}

impl PeakSet {
    /// Build every level from an interleaved buffer.
    pub fn build(interleaved: &[f32], channels: u16, base_bucket: usize) -> Self {
        let channels = channels.max(1) as usize;
        let base_bucket = base_bucket.max(1);
        let frames = interleaved.len() / channels;
        if frames == 0 {
            return Self {
                frames: 0,
                base_bucket,
                levels: Vec::new(),
            };
        }

        let bucket_count = frames.div_ceil(base_bucket);
        let mut buckets = Vec::with_capacity(bucket_count);
        for bucket in 0..bucket_count {
            let start = bucket * base_bucket;
            let end = ((bucket + 1) * base_bucket).min(frames);
            let mut min = f32::INFINITY;
            let mut max = f32::NEG_INFINITY;
            for frame in start..end {
                for channel in 0..channels {
                    let sample = interleaved[frame * channels + channel];
                    min = min.min(sample);
                    max = max.max(sample);
                }
            }
            buckets.push(Peak { min, max });
        }

        let mut levels = vec![PeakLevel {
            frames_per_bucket: base_bucket,
            buckets,
        }];

        // Coarser levels fold pairs of the previous level. `min`/`max` of a
        // pair is the min/max of the pair, so each level is one linear pass.
        while levels.last().is_some_and(|level| level.buckets.len() > 2) {
            let previous = levels.last().expect("level exists");
            let mut folded = Vec::with_capacity(previous.buckets.len().div_ceil(2));
            for pair in previous.buckets.chunks(2) {
                let min = pair.iter().fold(f32::INFINITY, |min, peak| min.min(peak.min));
                let max = pair
                    .iter()
                    .fold(f32::NEG_INFINITY, |max, peak| max.max(peak.max));
                folded.push(Peak { min, max });
            }
            levels.push(PeakLevel {
                frames_per_bucket: previous.frames_per_bucket.saturating_mul(2),
                buckets: folded,
            });
        }

        Self {
            frames,
            base_bucket,
            levels,
        }
    }

    /// Whether the set has no audio.
    pub fn is_empty(&self) -> bool {
        self.frames == 0 || self.levels.is_empty()
    }

    /// The level whose bucket count is closest to `target_buckets`.
    pub fn level_for_buckets(&self, target_buckets: usize) -> Option<&PeakLevel> {
        self.levels.iter().min_by_key(|level| {
            level.buckets.len().abs_diff(target_buckets.max(1))
        })
    }

    /// The level to draw when `visible_frames` are on screen across
    /// `screen_width` logical pixels.
    ///
    /// Picks the level whose bucket is closest to one bucket per two pixels, so
    /// zooming in uses finer data and zooming out uses coarser data without
    /// ever rescanning the source.
    pub fn level_for_span(&self, visible_frames: usize, screen_width: usize) -> Option<&PeakLevel> {
        let target_frames_per_bucket = (visible_frames / (screen_width.max(1) * 2).max(1)).max(1);
        self.levels.iter().min_by_key(|level| {
            level
                .frames_per_bucket
                .abs_diff(target_frames_per_bucket)
        })
    }

    /// Peak range across the whole sample, used to scale the drawing.
    pub fn overall_range(&self) -> Peak {
        let Some(finest) = self.levels.first() else {
            return Peak {
                min: -1.0,
                max: 1.0,
            };
        };
        let min = finest
            .buckets
            .iter()
            .fold(f32::INFINITY, |min, peak| min.min(peak.min));
        let max = finest
            .buckets
            .iter()
            .fold(f32::NEG_INFINITY, |max, peak| max.max(peak.max));
        if min.is_finite() && max.is_finite() && max > min {
            Peak { min, max }
        } else {
            Peak {
                min: -1.0,
                max: 1.0,
            }
        }
    }
}

/// Waveform viewport state for one sample.
#[derive(Debug, Clone, PartialEq)]
pub struct WaveformView {
    /// Horizontal zoom. `1.0` fits the whole sample.
    pub zoom: f32,
    /// Left edge as a fraction of the scrollable range, `0..=1`.
    pub scroll: f32,
    /// Selection in rendered seconds, ordered.
    pub selection: Option<(f32, f32)>,
    /// Playhead in rendered seconds.
    pub playhead: Option<f32>,
    /// Whether a drag is extending the selection.
    pub dragging: bool,
}

impl Default for WaveformView {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            scroll: 0.0,
            selection: None,
            playhead: None,
            dragging: false,
        }
    }
}

impl WaveformView {
    /// Visible duration at the current zoom.
    pub fn visible_seconds(&self, total: f32) -> f32 {
        (total / self.zoom.clamp(1.0, MAX_ZOOM)).max(0.0)
    }

    /// Left edge in seconds.
    pub fn left_seconds(&self, total: f32) -> f32 {
        let visible = self.visible_seconds(total);
        (total - visible).max(0.0) * self.scroll.clamp(0.0, 1.0)
    }

    /// Visible time range as `(start, end)`.
    pub fn range(&self, total: f32) -> (f32, f32) {
        let left = self.left_seconds(total);
        (left, (left + self.visible_seconds(total)).min(total))
    }

    /// Map a time to an x position in `0..=width`.
    pub fn time_to_x(&self, total: f32, time: f32, width: f32) -> f32 {
        let (left, right) = self.range(total);
        if right <= left || width <= 0.0 {
            return 0.0;
        }
        ((time - left) / (right - left) * width).clamp(-1.0e6, 1.0e6)
    }

    /// Map an x position back to a time.
    pub fn x_to_time(&self, total: f32, x: f32, width: f32) -> f32 {
        let (left, right) = self.range(total);
        if width <= 0.0 {
            return left;
        }
        (left + (x / width) * (right - left)).clamp(0.0, total)
    }

    /// Whether a time is inside the visible range.
    pub fn is_visible(&self, total: f32, time: f32) -> bool {
        let (left, right) = self.range(total);
        time >= left && time <= right
    }

    /// Zoom around a time anchor, keeping that time under the cursor.
    pub fn zoom_around(&mut self, total: f32, anchor: f32, factor: f32) {
        if total <= 0.0 || !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let old_range = self.range(total);
        let old_span = old_range.1 - old_range.0;
        if old_span <= 0.0 {
            return;
        }
        let anchor_fraction = ((anchor - old_range.0) / old_span).clamp(0.0, 1.0);
        self.zoom = (self.zoom * factor).clamp(1.0, MAX_ZOOM);

        let visible = self.visible_seconds(total);
        let scrollable = (total - visible).max(0.0);
        if scrollable <= 0.0 {
            self.scroll = 0.0;
            return;
        }
        let new_left = (anchor - anchor_fraction * visible).clamp(0.0, scrollable);
        self.scroll = new_left / scrollable;
    }

    /// Scroll so `time` is visible, centring it when it would leave the view.
    pub fn scroll_to(&mut self, total: f32, time: f32) {
        let visible = self.visible_seconds(total);
        let scrollable = (total - visible).max(0.0);
        if scrollable <= 0.0 {
            self.scroll = 0.0;
            return;
        }
        let left = self.left_seconds(total);
        if time >= left && time <= left + visible {
            return;
        }
        let target_left = (time - visible * 0.5).clamp(0.0, scrollable);
        self.scroll = target_left / scrollable;
    }

    /// Scroll by a fraction of the visible width. Positive moves right.
    pub fn scroll_by(&mut self, total: f32, fraction: f32) {
        let visible = self.visible_seconds(total);
        let scrollable = (total - visible).max(0.0);
        if scrollable <= 0.0 {
            self.scroll = 0.0;
            return;
        }
        let delta = visible * fraction;
        let left = (self.left_seconds(total) + delta).clamp(0.0, scrollable);
        self.scroll = left / scrollable;
    }

    /// Reset zoom and scroll to show the whole sample.
    pub fn fit(&mut self) {
        self.zoom = 1.0;
        self.scroll = 0.0;
    }

    /// Start a selection at `time`.
    pub fn begin_selection(&mut self, total: f32, time: f32) {
        let time = time.clamp(0.0, total);
        self.selection = Some((time, time));
        self.dragging = true;
    }

    /// Extend the in-progress selection to `time`.
    pub fn extend_selection(&mut self, total: f32, time: f32) {
        if !self.dragging {
            return;
        }
        let time = time.clamp(0.0, total);
        if let Some((start, _)) = self.selection {
            self.selection = Some((start.min(time), start.max(time)));
        }
    }

    /// Finish the in-progress selection. A zero-width drag becomes a click and
    /// clears the selection.
    pub fn finish_selection(&mut self) {
        self.dragging = false;
        if let Some((start, end)) = self.selection
            && (end - start).abs() < f32::EPSILON
        {
            self.selection = None;
        }
    }

    /// Select everything.
    pub fn select_all(&mut self, total: f32) {
        self.selection = Some((0.0, total));
    }

    /// Clear the selection.
    pub fn clear_selection(&mut self) {
        self.selection = None;
        self.dragging = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peaks_match_the_source_min_and_max() {
        let samples = vec![-0.5, 0.25, -0.1, 0.9, 0.0, 0.0, 0.2, 0.2];
        let peaks = PeakSet::build(&samples, 1, 4);
        assert_eq!(peaks.frames, 8);
        assert_eq!(peaks.levels[0].buckets.len(), 2);
        assert_eq!(
            peaks.levels[0].buckets[0],
            Peak {
                min: -0.5,
                max: 0.9
            }
        );
        assert_eq!(
            peaks.levels[0].buckets[1],
            Peak { min: 0.0, max: 0.2 }
        );
    }

    #[test]
    fn coarser_levels_fold_pairs_without_losing_extremes() {
        let samples: Vec<f32> = (0..64).map(|index| (index as f32 / 64.0) - 0.5).collect();
        let peaks = PeakSet::build(&samples, 1, 1);
        let finest = &peaks.levels[0];
        let coarse = &peaks.levels[1];
        assert_eq!(coarse.buckets.len(), finest.buckets.len() / 2);
        assert_eq!(
            coarse.buckets[0],
            Peak {
                min: finest.buckets[0].min.min(finest.buckets[1].min),
                max: finest.buckets[0].max.max(finest.buckets[1].max),
            }
        );
    }

    #[test]
    fn every_level_covers_the_same_frames() {
        let samples = vec![0.1; 1_000];
        let peaks = PeakSet::build(&samples, 1, 64);
        for level in &peaks.levels {
            let covered = level.buckets.len() * level.frames_per_bucket;
            assert!(covered >= peaks.frames);
            assert!(covered < peaks.frames + level.frames_per_bucket);
        }
    }

    #[test]
    fn the_coarsest_level_has_at_most_two_buckets() {
        let samples = vec![0.1; 100_000];
        let peaks = PeakSet::build(&samples, 1, 32);
        assert!(peaks.levels.last().expect("levels").buckets.len() <= 2);
    }

    #[test]
    fn stereo_peaks_take_the_extreme_of_both_channels() {
        let samples = vec![-0.25, 0.5, 0.75, -0.5];
        let peaks = PeakSet::build(&samples, 2, 1);
        assert_eq!(
            peaks.levels[0].buckets[0],
            Peak {
                min: -0.25,
                max: 0.5
            }
        );
        assert_eq!(
            peaks.levels[0].buckets[1],
            Peak {
                min: -0.5,
                max: 0.75
            },
            "the second frame spans both channels of its own frame"
        );

        // A bucket that spans both frames sees both channels.
        let wide = PeakSet::build(&samples, 2, 2);
        assert_eq!(
            wide.levels[0].buckets[0],
            Peak {
                min: -0.5,
                max: 0.75
            }
        );
    }

    #[test]
    fn empty_audio_produces_an_empty_set() {
        let peaks = PeakSet::build(&[], 2, 64);
        assert!(peaks.is_empty());
        assert_eq!(peaks.levels.len(), 0);
    }

    #[test]
    fn a_ten_second_sample_builds_quickly() {
        let frames = 48_000 * 10;
        let samples = vec![0.0_f32; frames];
        let start = std::time::Instant::now();
        let peaks = PeakSet::build(&samples, 1, BASE_BUCKET_FRAMES);
        let elapsed = start.elapsed();
        assert!(!peaks.is_empty());
        assert!(
            elapsed.as_millis() < 500,
            "peak build took {elapsed:?}"
        );
    }

    #[test]
    #[ignore = "10-minute peak build; run with --release"]
    fn a_ten_minute_sample_opens_in_under_100_ms_in_release() {
        let frames = 48_000 * 600;
        let samples = vec![0.0_f32; frames];
        let start = std::time::Instant::now();
        let peaks = PeakSet::build(&samples, 1, BASE_BUCKET_FRAMES);
        let elapsed = start.elapsed();
        assert!(!peaks.is_empty());
        assert!(
            elapsed.as_millis() < 100,
            "peak build took {elapsed:?}, expected under 100 ms"
        );
    }

    #[test]
    fn level_selection_tracks_the_requested_bucket_count() {
        let samples = vec![0.1; 10_000];
        let peaks = PeakSet::build(&samples, 1, 10);
        let level = peaks.level_for_buckets(100).expect("level");
        assert!((level.buckets.len() as isize - 100).unsigned_abs() < 60);
    }

    #[test]
    fn the_default_view_fits_the_sample() {
        let view = WaveformView::default();
        assert_eq!(view.range(10.0), (0.0, 10.0));
        assert_eq!(view.time_to_x(10.0, 5.0, 500.0), 250.0);
        assert_eq!(view.x_to_time(10.0, 250.0, 500.0), 5.0);
    }

    #[test]
    fn time_and_x_round_trip_at_any_zoom() {
        let mut view = WaveformView::default();
        view.zoom = 8.0;
        view.scroll = 0.33;
        let total = 12.0;
        for x in [0.0, 17.0, 250.0, 799.0] {
            let time = view.x_to_time(total, x, 800.0);
            let back = view.time_to_x(total, time, 800.0);
            assert!((back - x).abs() < 0.01, "{x} -> {time} -> {back}");
        }
    }

    #[test]
    fn zoom_keeps_the_anchor_time_under_the_cursor() {
        let mut view = WaveformView::default();
        view.zoom = 4.0;
        view.scroll = 0.25;
        let total = 40.0;
        let anchor = view.x_to_time(total, 300.0, 1_000.0);
        let before = view.time_to_x(total, anchor, 1_000.0);
        view.zoom_around(total, anchor, 2.0);
        let after = view.time_to_x(total, anchor, 1_000.0);
        assert!((before - after).abs() < 1.0, "anchor moved {before} -> {after}");
    }

    #[test]
    fn zoom_is_clamped_to_the_supported_range() {
        let mut view = WaveformView::default();
        view.zoom_around(10.0, 5.0, 1.0e9);
        assert_eq!(view.zoom, MAX_ZOOM);
        view.zoom_around(10.0, 5.0, 1.0e-9);
        assert_eq!(view.zoom, 1.0);
        assert_eq!(view.scroll, 0.0);
    }

    #[test]
    fn scroll_to_brings_an_off_screen_time_into_view() {
        let mut view = WaveformView::default();
        view.zoom = 10.0;
        view.scroll = 0.0;
        view.scroll_to(100.0, 80.0);
        assert!(view.is_visible(100.0, 80.0));
        let (left, right) = view.range(100.0);
        assert!(left > 0.0 && right <= 100.0);
    }

    #[test]
    fn scroll_by_moves_a_fraction_of_the_visible_window() {
        let mut view = WaveformView::default();
        view.zoom = 10.0;
        let before = view.range(100.0);
        view.scroll_by(100.0, 0.5);
        let after = view.range(100.0);
        assert!((after.0 - before.0 - 5.0).abs() < 0.001);
    }

    #[test]
    fn a_drag_selection_is_ordered_and_a_click_clears_it() {
        let mut view = WaveformView::default();
        view.begin_selection(10.0, 7.0);
        view.extend_selection(10.0, 3.0);
        view.finish_selection();
        assert_eq!(view.selection, Some((3.0, 7.0)));

        view.begin_selection(10.0, 5.0);
        view.finish_selection();
        assert_eq!(view.selection, None);
    }

    #[test]
    fn empty_or_inverted_audio_is_handled_without_panicking() {
        let mut view = WaveformView::default();
        view.zoom_around(0.0, 0.0, 2.0);
        view.scroll_to(0.0, 0.0);
        view.scroll_by(0.0, 1.0);
        view.begin_selection(0.0, 0.0);
        view.extend_selection(0.0, 0.0);
        view.finish_selection();
        assert_eq!(view.range(0.0), (0.0, 0.0));
    }

    #[test]
    fn overall_range_falls_back_to_full_scale_for_silence() {
        let peaks = PeakSet::build(&[0.0; 10], 1, 4);
        assert_eq!(peaks.overall_range(), Peak { min: -1.0, max: 1.0 });
    }
}
