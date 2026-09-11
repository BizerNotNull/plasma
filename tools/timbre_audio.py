"""Local-only target loading, compact descriptors, and phase-insensitive fitting loss.

Dependencies: numpy, soundfile, scipy. No normalization is applied to audio: the
absolute level and any trailing silence remain meaningful fitting targets.
"""

import math
from pathlib import Path

import numpy as np
import soundfile as sf
from scipy.signal import resample_poly


_MAX_SECONDS = 30.0
_MIN_SECONDS = 0.05
_SILENCE_RMS = 1e-8
_EPS = 1e-12


def _sample_rate(value):
    if isinstance(value, (bool, np.bool_)) or not isinstance(value, (int, np.integer)):
        raise ValueError("sample_rate must be an integer between 8000 and 192000 Hz")
    if not 8000 <= value <= 192000:
        raise ValueError("sample_rate must be between 8000 and 192000 Hz")
    return int(value)


def _audio(audio, sample_rate):
    sample_rate = _sample_rate(sample_rate)
    data = np.asarray(audio, dtype=np.float64)
    if data.ndim != 2 or data.shape[1] != 2:
        raise ValueError(
            "audio must have shape (frames, 2); use load_target for mono files"
        )
    if len(data) < math.ceil(_MIN_SECONDS * sample_rate):
        raise ValueError("audio is too short: at least 50 ms is required")
    if len(data) > math.floor(_MAX_SECONDS * sample_rate):
        raise ValueError("audio exceeds the 30-second duration limit")
    if not np.isfinite(data).all():
        raise ValueError("audio contains NaN or infinity")
    if np.max(np.abs(data)) > np.finfo(np.float32).max:
        raise ValueError("audio amplitude exceeds the float32 audio range")
    return data


def load_target(path, sample_rate=48000):
    """Decode mono/stereo audio supported by soundfile into float64 stereo.

    Resampling uses a deterministic polyphase FIR with antialias filtering.
    Rejects multichannel, nonfinite, silent, >30-second (before trimming), or
    <50-ms inputs. Leading silence is detected independently of stereo polarity
    at max(1e-7, peak * 0.001), keeping 2 ms of preroll. Trailing silence is
    NEVER trimmed. A retained sample shorter than 50 ms is rejected too.

    Returns (audio, metadata). Metadata keys: source_path, source_sample_rate,
    source_channels, source_frames, source_duration_seconds, sample_rate,
    frames, duration_seconds, onset_trim_frames, onset_trim_seconds,
    trailing_silence_preserved. Trim frames refer to the output sample rate.
    """
    sample_rate = _sample_rate(sample_rate)
    path = Path(path)
    try:
        with sf.SoundFile(str(path)) as source:
            source_rate = int(source.samplerate)
            source_channels = int(source.channels)
            source_frames = int(source.frames)
            if source_channels not in (1, 2):
                raise ValueError(
                    f"unsupported channel count {source_channels}; use mono or stereo"
                )
            if source_rate <= 0:
                raise ValueError("input has an invalid sample rate")
            duration = source_frames / source_rate
            if duration > _MAX_SECONDS:
                raise ValueError(
                    "input exceeds the 30-second limit before onset trimming"
                )
            if duration < _MIN_SECONDS:
                raise ValueError("input is too short: at least 50 ms is required")
            data = source.read(dtype="float64", always_2d=True)
    except (RuntimeError, OSError) as error:
        raise ValueError(f"cannot decode audio {path}: {error}") from error
    if not np.isfinite(data).all():
        raise ValueError("input audio contains NaN or infinity")
    if np.max(np.abs(data)) > np.finfo(np.float32).max:
        raise ValueError("input amplitude exceeds the float32 audio range")
    if float(np.sqrt(np.mean(data * data))) <= _SILENCE_RMS:
        raise ValueError("input audio is silent (RMS <= 1e-8)")
    if source_rate != sample_rate:
        divisor = math.gcd(source_rate, sample_rate)
        data = resample_poly(
            data, sample_rate // divisor, source_rate // divisor, axis=0
        )
    if source_channels == 1:
        data = np.repeat(data, 2, axis=1)
    amplitude = np.max(np.abs(data), axis=1)
    threshold = max(1e-7, float(np.max(amplitude)) * 0.001)
    active = np.flatnonzero(amplitude >= threshold)
    if not len(active):
        raise ValueError(
            "input is silent or too quiet to locate an onset (peak < 1e-7)"
        )
    trim = max(0, int(active[0]) - round(0.002 * sample_rate))
    data = np.ascontiguousarray(_audio(data[trim:], sample_rate))
    metadata = {
        "source_path": str(path),
        "source_sample_rate": source_rate,
        "source_channels": source_channels,
        "source_frames": source_frames,
        "source_duration_seconds": duration,
        "sample_rate": sample_rate,
        "frames": int(len(data)),
        "duration_seconds": len(data) / sample_rate,
        "onset_trim_frames": trim,
        "onset_trim_seconds": trim / sample_rate,
        "trailing_silence_preserved": True,
    }
    return data, metadata


def _envelope(data, sample_rate):
    """Per-channel, nonoverlapping 10-ms RMS, including the partial last bin."""
    block = max(1, round(sample_rate * 0.01))
    starts = np.arange(0, len(data), block)
    counts = np.minimum(block, len(data) - starts)
    return np.sqrt(np.add.reduceat(data * data, starts, axis=0) / counts[:, None])


def _magnitude(data, size):
    """Root-mean-square channel STFT magnitude; anti-phase audio cannot cancel."""
    hop = size // 4
    padded = np.pad(data, ((size // 2, size // 2), (0, 0)))
    window = np.hanning(size)
    power = None
    for channel in range(2):
        frames = np.lib.stride_tricks.sliding_window_view(padded[:, channel], size)[
            ::hop
        ]
        spectrum = np.fft.rfft(frames * window, axis=1)
        channel_power = spectrum.real**2 + spectrum.imag**2
        if power is None:
            power = channel_power
        else:
            power += channel_power
    return np.sqrt(power * 0.5) / np.sum(window)


def _pitch(data, sample_rate):
    """Normalized autocorrelation of the loudest 350-ms region, 30..2000 Hz.

    The strongest channel avoids mid-channel cancellation. Near-equal periodic
    peaks prefer the shortest lag; fewer than 2.5 periods or weak periodicity
    yields explicit None. Reliability is periodicity, not a calibrated posterior.
    """
    energy = np.mean(data * data, axis=0)
    if float(np.max(energy)) <= _SILENCE_RMS**2:
        return None, 0.0
    signal = data[:, int(np.argmax(energy))]
    factor = max(1, sample_rate // 12000)
    if factor > 1:
        signal = resample_poly(signal, 1, factor)
    rate = sample_rate / factor
    width = min(len(signal), round(0.35 * rate))
    hop = max(1, round(0.05 * rate))
    starts = np.arange(0, len(signal) - width + 1, hop)
    integral = np.concatenate(([0.0], np.cumsum(signal * signal)))
    powers = integral[starts + width] - integral[starts]
    start = int(starts[int(np.argmax(powers))])
    signal = signal[start : start + width].copy()
    signal -= np.mean(signal)
    energy = float(np.dot(signal, signal))
    if energy <= _SILENCE_RMS**2 * width:
        return None, 0.0
    fft_size = 1 << (2 * width - 1).bit_length()
    spectrum = np.fft.rfft(signal, fft_size)
    correlation = np.fft.irfft(np.abs(spectrum) ** 2, fft_size)[:width]
    low = max(2, math.ceil(rate / 2000))
    high = min(width - 2, math.floor(rate / 30), math.floor(width / 2.5))
    if high <= low:
        return None, 0.0
    lags = np.arange(high + 2)
    cumulative = np.concatenate(([0.0], np.cumsum(signal * signal)))
    denominator = np.sqrt(
        cumulative[width - lags] * (cumulative[width] - cumulative[lags])
    )
    correlation = correlation[: high + 2] / np.maximum(denominator, _EPS)
    peaks = (
        np.flatnonzero(
            (correlation[low : high + 1] > correlation[low - 1 : high])
            & (correlation[low : high + 1] >= correlation[low + 1 : high + 2])
        )
        + low
    )
    if not len(peaks):
        return None, 0.0
    best = float(np.clip(np.max(correlation[peaks]), 0.0, 1.0))
    if best < 0.75:
        return None, best
    lag = int(peaks[np.flatnonzero(correlation[peaks] >= max(0.75, best * 0.92))[0]])
    left, center, right = correlation[lag - 1 : lag + 2]
    curvature = left - 2 * center + right
    adjustment = 0.0 if abs(curvature) < _EPS else 0.5 * (left - right) / curvature
    frequency = rate / (lag + float(np.clip(adjustment, -0.5, 0.5)))
    if not 30 <= frequency <= 2000:
        return None, best
    return float(frequency), float(np.clip(center, 0.0, 1.0))


def _stereo(data):
    left, right = data[:, 0], data[:, 1]
    left_power = float(np.mean(left * left))
    right_power = float(np.mean(right * right))
    cross = float(np.mean(left * right))
    total = left_power + right_power
    return {
        "correlation": float(
            np.clip(cross / max(math.sqrt(left_power * right_power), _EPS), -1, 1)
        ),
        "side_energy_fraction": float(
            np.clip((total - 2 * cross) / max(2 * total, _EPS), 0, 1)
        ),
        "balance": (right_power - left_power) / max(total, _EPS),
    }


def analyze(audio, sample_rate):
    """Return compact JSON-safe numeric note, level, envelope, and stereo data.

    Input must be finite stereo frames x 2, 50 ms..30 seconds. Silent candidates
    are valid here (unlike load_target). frequency_hz is None when unreliable.
    spectrum_band_edges_hz and spectrum_band_energy describe relative energy;
    envelope_rms has 32 equal-duration bins and retains absolute loudness.
    """
    data = _audio(audio, sample_rate)
    rms = float(np.sqrt(np.mean(data * data)))
    peak = float(np.max(np.abs(data)))
    frequency, reliability = _pitch(data, sample_rate)
    magnitude = _magnitude(data, 4096)
    power = np.mean(magnitude * magnitude, axis=0)
    frequencies = np.fft.rfftfreq(4096, 1 / sample_rate)
    total = float(np.sum(power))
    edges = (
        [0.0]
        + [
            float(edge)
            for edge in (63, 125, 250, 500, 1000, 2000, 4000, 8000, 16000)
            if edge < sample_rate / 2
        ]
        + [sample_rate / 2]
    )
    bands = []
    for index, (low, high) in enumerate(zip(edges, edges[1:])):
        mask = (frequencies >= low) & (
            (frequencies <= high) if index == len(edges) - 2 else (frequencies < high)
        )
        bands.append(float(np.sum(power[mask]) / max(total, _EPS)))
    envelope = [
        float(np.sqrt(np.mean(part * part))) for part in np.array_split(data, 32)
    ]
    return {
        "frequency_hz": frequency,
        "pitch_reliability": reliability,
        "duration_seconds": len(data) / sample_rate,
        "rms": rms,
        "peak": peak,
        "crest_factor": peak / max(rms, _EPS),
        "envelope_rms": envelope,
        "envelope_peak_seconds": (int(np.argmax(envelope)) + 0.5)
        * len(data)
        / (32 * sample_rate),
        "spectral_centroid_hz": float(np.dot(frequencies, power) / max(total, _EPS)),
        "spectral_flatness": float(
            np.exp(np.mean(np.log(power + _EPS))) / (np.mean(power) + _EPS)
        )
        if total > _EPS
        else 0.0,
        "spectrum_band_edges_hz": edges,
        "spectrum_band_energy": bands,
        "stereo": _stereo(data),
    }


def compare(target, candidate, sample_rate):
    """Return finite lower-is-better total and component losses (identity = 0).

    Arrays must have exactly equal stereo shapes and aligned onsets. No gain,
    time, or phase alignment is performed. Three STFT resolutions compare
    magnitude, not waveform phase; 10-ms channel RMS compares articulation and
    panning over time. Stereo moments distinguish width and anti-phase content.
    Losses preserve absolute loudness. Target data is never implicitly cached
    or mutated, so callers may safely reuse or change their own arrays.
    """
    target = _audio(target, sample_rate)
    candidate = _audio(candidate, sample_rate)
    if target.shape != candidate.shape:
        raise ValueError(
            "target and candidate must have exactly the same (frames, 2) shape"
        )
    linear_losses, log_losses = [], []
    for size in (512, 2048, 8192):
        reference = _magnitude(target, size)
        actual = _magnitude(candidate, size)
        linear_losses.append(
            float(
                np.linalg.norm(actual - reference)
                / max(float(np.linalg.norm(reference)), 1e-7)
            )
        )
        scale = max(float(np.max(reference)), 1e-7)
        reference_log = np.log1p(100 * reference / scale)
        actual_log = np.log1p(100 * actual / scale)
        log_losses.append(
            float(
                np.mean(np.abs(actual_log - reference_log))
                / max(float(np.mean(reference_log)), 0.1)
            )
        )
    reference_envelope = _envelope(target, sample_rate)
    actual_envelope = _envelope(candidate, sample_rate)
    envelope = float(
        np.mean(np.abs(actual_envelope - reference_envelope))
        / max(float(np.mean(reference_envelope)), 1e-7)
    )
    reference_rms = float(np.sqrt(np.mean(target * target)))
    actual_rms = float(np.sqrt(np.mean(candidate * candidate)))
    level = abs(math.log((actual_rms + 1e-8) / (reference_rms + 1e-8)))
    reference_stereo, actual_stereo = _stereo(target), _stereo(candidate)
    stereo = (
        sum(abs(reference_stereo[key] - actual_stereo[key]) for key in reference_stereo)
        / 3
    )
    spectral_magnitude = float(np.mean(linear_losses))
    spectral_log = float(np.mean(log_losses))
    total = (
        spectral_magnitude
        + 0.5 * spectral_log
        + 0.75 * envelope
        + 0.25 * level
        + 0.25 * stereo
    )
    return {
        "total": float(total),
        "spectral_magnitude": spectral_magnitude,
        "spectral_log": spectral_log,
        "envelope": envelope,
        "level": float(level),
        "stereo": float(stereo),
    }
