//! Radix-2 FFT and spectrum helpers.
//!
//! The evaluation metrics need the cadence signature of an accelerometer trace
//! and the step ripple of a barometer trace. That is a small, fixed-size
//! transform, so it is implemented here instead of adding a dependency.

/// In-place iterative radix-2 Cooley–Tukey FFT.
#[derive(Debug, Clone)]
pub struct Fft {
    size: usize,
    reversed: Vec<usize>,
    cos: Vec<f64>,
    sin: Vec<f64>,
}

impl Fft {
    /// Prepares a transform of `size` samples.
    ///
    /// `size` must be a power of two; the next power of two is used otherwise.
    pub fn new(size: usize) -> Self {
        let size = size.max(2).next_power_of_two();
        let bits = size.trailing_zeros();
        let mut reversed = vec![0usize; size];
        for (index, slot) in reversed.iter_mut().enumerate() {
            *slot = index.reverse_bits() >> (usize::BITS - bits);
        }
        let half = size / 2;
        let mut cos = vec![0.0; half];
        let mut sin = vec![0.0; half];
        for k in 0..half {
            let angle = -std::f64::consts::TAU * k as f64 / size as f64;
            cos[k] = angle.cos();
            sin[k] = angle.sin();
        }
        Self {
            size,
            reversed,
            cos,
            sin,
        }
    }

    /// Transform size in samples.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Transforms a real signal in place; the input is zero-padded or truncated
    /// to the transform size.
    pub fn forward_real(&self, signal: &[f64]) -> Vec<(f64, f64)> {
        let mut data: Vec<(f64, f64)> = vec![(0.0, 0.0); self.size];
        for (index, value) in signal.iter().take(self.size).enumerate() {
            data[index].0 = *value;
        }
        for index in 0..self.size {
            let j = self.reversed[index];
            if index < j {
                data.swap(index, j);
            }
        }
        let mut len = 2;
        while len <= self.size {
            let half = len / 2;
            let step = self.size / len;
            for start in (0..self.size).step_by(len) {
                for k in 0..half {
                    let (wr, wi) = (self.cos[k * step], self.sin[k * step]);
                    let (ar, ai) = data[start + k];
                    let (br, bi) = data[start + k + half];
                    let tr = br * wr - bi * wi;
                    let ti = br * wi + bi * wr;
                    data[start + k] = (ar + tr, ai + ti);
                    data[start + k + half] = (ar - tr, ai - ti);
                }
            }
            len *= 2;
        }
        data
    }
}

/// One peak of a spectrum.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpectrumPeak {
    /// Frequency in Hz.
    pub frequency_hz: f64,
    /// Magnitude at that frequency.
    pub magnitude: f64,
}

/// Single-sided amplitude spectrum of a real signal.
#[derive(Debug, Clone, PartialEq)]
pub struct Spectrum {
    /// Frequency of each bin in Hz.
    pub frequencies_hz: Vec<f64>,
    /// Amplitude at each bin.
    pub magnitudes: Vec<f64>,
    /// Frequency resolution in Hz.
    pub resolution_hz: f64,
}

impl Spectrum {
    /// Computes the single-sided amplitude spectrum of `signal` sampled at
    /// `sample_rate_hz`.
    ///
    /// A Hann window is applied first: the cadence peaks of interest sit next to
    /// a much larger low-frequency component, and an unwindowed transform would
    /// smear them into it.
    ///
    /// The window spans the *signal*, not the padded transform. Building it over
    /// the padded length leaves the last real sample untapered — a step into the
    /// zero padding, which smears the line it is meant to isolate — and it also
    /// breaks the amplitude normalisation, because the coherent gain that sets
    /// the scale is the window's mean over the samples actually multiplied. A
    /// 1500-sample tone in a 2048-point transform reads about 0.73 of its true
    /// amplitude with the padded window and 1.0 with this one.
    #[allow(clippy::needless_range_loop)]
    pub fn of(signal: &[f64], sample_rate_hz: f64) -> Self {
        if signal.is_empty() {
            // No samples, no spectrum. Everything downstream reads a magnitude at a
            // frequency, and an empty bin list answers every such query with "not
            // measured" instead of inventing a zero line.
            return Self {
                frequencies_hz: Vec::new(),
                magnitudes: Vec::new(),
                resolution_hz: sample_rate_hz,
            };
        }
        let fft = Fft::new(signal.len());
        let size = fft.size();
        let count = signal.len().clamp(1, size);
        let window: Vec<f64> = if count < 2 {
            vec![1.0; count]
        } else {
            (0..count)
                .map(|index| {
                    0.5 - 0.5 * (std::f64::consts::TAU * index as f64 / count as f64).cos()
                })
                .collect()
        };
        let mut windowed = vec![0.0f64; size];
        for index in 0..count {
            windowed[index] = signal[index] * window[index];
        }
        let transformed = fft.forward_real(&windowed);

        let bins = size / 2 + 1;
        let resolution_hz = sample_rate_hz / size as f64;
        // Amplitude of a line of amplitude `A`: `A = 2 |X_k| / sum(w)`, since the
        // windowed line puts `A/2 sum(w)` into its positive-frequency bin.
        let window_sum: f64 = window.iter().sum();
        let normalisation = if window_sum > 0.0 {
            2.0 / window_sum
        } else {
            0.0
        };
        let mut frequencies_hz = Vec::with_capacity(bins);
        let mut magnitudes = Vec::with_capacity(bins);
        for bin in 0..bins {
            let (re, im) = transformed[bin];
            frequencies_hz.push(bin as f64 * resolution_hz);
            magnitudes.push((re * re + im * im).sqrt() * normalisation);
        }
        Self {
            frequencies_hz,
            magnitudes,
            resolution_hz,
        }
    }

    /// Highest peaks, sorted by descending magnitude.
    pub fn peaks(&self, count: usize, min_frequency_hz: f64) -> Vec<SpectrumPeak> {
        let mut candidates: Vec<SpectrumPeak> = (1..self.magnitudes.len().saturating_sub(1))
            .filter(|index| self.frequencies_hz[*index] >= min_frequency_hz)
            .filter(|index| {
                self.magnitudes[*index] >= self.magnitudes[*index - 1]
                    && self.magnitudes[*index] >= self.magnitudes[*index + 1]
            })
            .map(|index| SpectrumPeak {
                frequency_hz: self.frequencies_hz[index],
                magnitude: self.magnitudes[index],
            })
            .collect();
        candidates.sort_by(|a, b| {
            b.magnitude
                .partial_cmp(&a.magnitude)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        candidates.truncate(count);
        candidates
    }

    /// Magnitude at the bin closest to `frequency_hz`.
    pub fn magnitude_at(&self, frequency_hz: f64) -> f64 {
        if self.resolution_hz <= 0.0 {
            return 0.0;
        }
        let bin = (frequency_hz / self.resolution_hz).round() as usize;
        self.magnitudes.get(bin).copied().unwrap_or(0.0)
    }

    /// Strongest peak within `[low, high]`.
    pub fn peak_in_band(&self, low: f64, high: f64) -> Option<SpectrumPeak> {
        let mut best: Option<SpectrumPeak> = None;
        for (index, frequency) in self.frequencies_hz.iter().enumerate() {
            if *frequency < low || *frequency > high {
                continue;
            }
            let magnitude = self.magnitudes[index];
            if best.map(|peak| magnitude > peak.magnitude).unwrap_or(true) {
                best = Some(SpectrumPeak {
                    frequency_hz: *frequency,
                    magnitude,
                });
            }
        }
        best
    }
}
