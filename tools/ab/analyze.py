#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "scipy"]
# ///
"""A/B analysis for two recordings of the same passage (see passage.py
and README.md): loudness-match, then report peak/RMS, spectral centroid
over time, a release-splice discontinuity metric, and the noise floor
between notes, for each file.

Reads raw WAV bytes directly rather than the stdlib `wave` module,
which cannot open GrandOrgue's IEEE-float (format tag 3) files.

Usage:
    uv run tools/ab/analyze.py go-take.wav aristide-take.wav
"""

import struct
import sys
from dataclasses import dataclass

import numpy as np
from scipy.signal import stft


@dataclass
class Wav:
    path: str
    sr: int
    channels: int
    data: np.ndarray  # shape (n_frames, n_channels), float32 in [-1, 1]


def read_wav(path: str) -> Wav:
    with open(path, "rb") as f:
        riff = f.read(12)
        if riff[:4] != b"RIFF" or riff[8:12] != b"WAVE":
            raise ValueError(f"{path}: not a RIFF/WAVE file")
        fmt_tag = channels = sr = bits = None
        data_bytes = None
        while True:
            hdr = f.read(8)
            if len(hdr) < 8:
                break
            cid, size = struct.unpack("<4sI", hdr)
            body_start = f.tell()
            if cid == b"fmt ":
                fmt = f.read(size)
                fmt_tag, channels, sr, _byte_rate, _align, bits = struct.unpack("<HHIIHH", fmt[:16])
            elif cid == b"data":
                # A file whose recorder process was killed hard (SIGTERM,
                # not the graceful SIGINT path) can leave this chunk's
                # declared size at 0 even though the bytes are on disk --
                # trust EOF over a zero/short declared size.
                declared = size
                remaining_in_file = _file_size(f) - body_start
                read_size = declared if 0 < declared <= remaining_in_file else remaining_in_file
                data_bytes = f.read(read_size)
                break
            else:
                f.seek(size, 1)
            f.seek(body_start + size + (size & 1), 0)
    if fmt_tag is None or data_bytes is None:
        raise ValueError(f"{path}: missing fmt or data chunk")

    if fmt_tag == 3 and bits == 32:  # IEEE float32
        arr = np.frombuffer(data_bytes, dtype="<f4")
    elif fmt_tag == 1 and bits == 16:  # PCM16
        arr = np.frombuffer(data_bytes, dtype="<i2").astype(np.float32) / 32768.0
    elif fmt_tag == 1 and bits == 32:  # PCM32
        arr = np.frombuffer(data_bytes, dtype="<i4").astype(np.float32) / 2147483648.0
    else:
        raise ValueError(f"{path}: unsupported format tag={fmt_tag} bits={bits}")

    n = (len(arr) // channels) * channels
    arr = arr[:n].reshape(-1, channels)
    return Wav(path=path, sr=sr, channels=channels, data=arr)


def _file_size(f) -> int:
    pos = f.tell()
    f.seek(0, 2)
    size = f.tell()
    f.seek(pos, 0)
    return size


def dbfs(x: float) -> float:
    return 20.0 * np.log10(max(x, 1e-12))


def mono(w: Wav) -> np.ndarray:
    return w.data.mean(axis=1)


def loudness_match_gain(w: Wav, target_dbfs: float = -20.0) -> float:
    """Broadband RMS over non-silent frames only (> -60 dBFS), so the
    long lead-in/tail silence in each take doesn't drag the reference
    level down and produce a misleadingly large matching gain."""
    m = mono(w)
    frame = max(1, w.sr // 20)  # 50 ms frames
    n_frames = len(m) // frame
    if n_frames == 0:
        return 1.0
    frames = m[: n_frames * frame].reshape(n_frames, frame)
    frame_rms = np.sqrt(np.mean(frames**2, axis=1))
    voiced = frame_rms[frame_rms > 1e-3]  # > -60 dBFS
    if len(voiced) == 0:
        return 1.0
    ref_rms = float(np.sqrt(np.mean(voiced**2)))
    target_rms = 10 ** (target_dbfs / 20.0)
    return target_rms / max(ref_rms, 1e-9)


def peak_rms(w: Wav, gain: float) -> tuple[float, float]:
    x = w.data * gain
    peak = float(np.max(np.abs(x)))
    rms = float(np.sqrt(np.mean(x**2)))
    return dbfs(peak), dbfs(rms)


def spectral_centroid_series(w: Wav, gain: float) -> np.ndarray:
    x = mono(w) * gain
    nperseg = 2048
    noverlap = nperseg - 512
    f, _t, Zxx = stft(x, fs=w.sr, nperseg=nperseg, noverlap=noverlap, window="hann")
    mag = np.abs(Zxx)
    energy = mag.sum(axis=0)
    centroid = np.divide(
        (f[:, None] * mag).sum(axis=0), energy, out=np.zeros_like(energy), where=energy > 1e-9
    )
    voiced = centroid[energy > energy.max() * 0.01]
    return voiced


def discontinuity_metric(w: Wav, gain: float, window_ms: float = 8.0, threshold: float = 10.0):
    """Second-difference spikes relative to a local RMS envelope, per
    channel -- the click/splice-kink probe (see the max-step idiom in
    crates/aristide-engine/src/tests/swell.rs, extended to a curvature
    measure so a fast-but-smooth attack doesn't false-positive).

    A spike is a sample where |second_diff| exceeds `threshold` times
    the local RMS of the second-difference signal in a `window_ms`
    window around it. Returns (spike_count_per_channel, top_ratios).
    """
    win = max(3, int(w.sr * window_ms / 1000))
    results = []
    for ch in range(w.channels):
        x = w.data[:, ch] * gain
        d2 = x[2:] - 2 * x[1:-1] + x[:-2]
        # Local RMS envelope of the second difference via a moving
        # average of d2^2 (uniform filter, edge-padded).
        d2sq = d2**2
        kernel = np.ones(win) / win
        local_ms = np.convolve(d2sq, kernel, mode="same")
        local_rms = np.sqrt(np.maximum(local_ms, 1e-14))
        ratio = np.abs(d2) / local_rms
        spikes = ratio > threshold
        count = int(spikes.sum())
        top = np.sort(ratio)[-5:][::-1].tolist() if len(ratio) else []
        results.append((count, top))
    return results


def noise_floor(w: Wav, gain: float, window_ms: float = 200.0) -> float:
    """The quietest window's RMS, in dBFS -- the between-notes hiss/
    room-noise floor. Digital silence (true zero, e.g. an unstarted
    lead-in) is excluded so the number reflects recorded noise, not
    an empty buffer."""
    m = mono(w) * gain
    win = max(1, int(w.sr * window_ms / 1000))
    n_windows = len(m) // win
    if n_windows == 0:
        return dbfs(0.0)
    frames = m[: n_windows * win].reshape(n_windows, win)
    rms = np.sqrt(np.mean(frames**2, axis=1))
    nonzero = rms[rms > 1e-6]
    if len(nonzero) == 0:
        return dbfs(0.0)
    return dbfs(float(np.min(nonzero)))


def analyze(path: str) -> dict:
    w = read_wav(path)
    gain = loudness_match_gain(w)
    peak_db, rms_db = peak_rms(w, gain)
    centroid = spectral_centroid_series(w, gain)
    disc = discontinuity_metric(w, gain)
    floor_db = noise_floor(w, gain)
    duration = len(w.data) / w.sr
    return {
        "path": path,
        "sr": w.sr,
        "channels": w.channels,
        "duration_s": duration,
        "loudness_match_gain_db": dbfs(gain) - dbfs(1.0),
        "peak_dbfs": peak_db,
        "rms_dbfs": rms_db,
        "centroid_mean_hz": float(np.mean(centroid)) if len(centroid) else 0.0,
        "centroid_std_hz": float(np.std(centroid)) if len(centroid) else 0.0,
        "centroid_p10_p90_hz": (
            (float(np.percentile(centroid, 10)), float(np.percentile(centroid, 90)))
            if len(centroid)
            else (0.0, 0.0)
        ),
        "discontinuities_per_channel": [c for c, _ in disc],
        "discontinuity_top_ratios": [top for _, top in disc],
        "noise_floor_dbfs": floor_db,
    }


def print_report(reports: list[dict]) -> None:
    fields = [
        ("duration_s", "Duration (s)", "{:.2f}"),
        ("loudness_match_gain_db", "Loudness-match gain (dB)", "{:+.2f}"),
        ("peak_dbfs", "Peak (dBFS, matched)", "{:.2f}"),
        ("rms_dbfs", "RMS (dBFS, matched)", "{:.2f}"),
        ("centroid_mean_hz", "Spectral centroid mean (Hz)", "{:.0f}"),
        ("centroid_std_hz", "Spectral centroid std (Hz)", "{:.0f}"),
        ("noise_floor_dbfs", "Noise floor (dBFS, matched)", "{:.2f}"),
    ]
    names = [r["path"].split("/")[-1] for r in reports]
    colw = max(28, *(len(n) for n in names)) + 2
    print()
    print(f"{'metric':32}" + "".join(f"{n:>{colw}}" for n in names))
    print("-" * (32 + colw * len(names)))
    for key, label, fmt in fields:
        row = f"{label:32}"
        for r in reports:
            row += f"{fmt.format(r[key]):>{colw}}"
        print(row)
    print()
    for r in reports:
        dur = r["duration_s"]
        print(f"{r['path']}: discontinuity spikes per channel (ratio > 10x local level):")
        for ch, (count, top) in enumerate(zip(r["discontinuities_per_channel"], r["discontinuity_top_ratios"])):
            top_str = ", ".join(f"{t:.1f}x" for t in top)
            rate = count / dur if dur > 0 else 0.0
            print(f"  ch{ch}: {count} spikes ({rate:.2f}/s, durations differ -- rate is the fair comparison), top ratios: {top_str}")
    print()
    for r in reports:
        lo, hi = r["centroid_p10_p90_hz"]
        print(f"{r['path']}: spectral centroid p10-p90 = {lo:.0f}-{hi:.0f} Hz")


def main() -> None:
    if len(sys.argv) < 3:
        print("usage: analyze.py FILE1.wav FILE2.wav [...]", file=sys.stderr)
        raise SystemExit(2)
    reports = [analyze(p) for p in sys.argv[1:]]
    print_report(reports)


if __name__ == "__main__":
    main()
