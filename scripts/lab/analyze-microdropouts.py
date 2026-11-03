"""Inspect captured PCM without concealing missing AVFoundation timestamp ranges."""
import json
from fractions import Fraction
from pathlib import Path
import subprocess

import numpy as np


def probe(path: Path) -> dict:
    return json.loads(subprocess.check_output([
        "ffprobe", "-v", "error", "-show_streams", "-show_packets",
        "-show_entries", "stream=index,sample_rate,channels,start_time,time_base:packet=stream_index,pts,duration,size",
        "-of", "json", str(path),
    ]))


def runs(mask: np.ndarray) -> list[tuple[int, int]]:
    boundaries = np.flatnonzero(np.diff(np.r_[False, mask, False]))
    return [(int(a), int(b - a)) for a, b in boundaries.reshape(-1, 2)]


def analyze(path: Path) -> dict:
    metadata = probe(path)
    result = {"streams": []}
    for stream in metadata["streams"]:
        index, rate, channels = stream["index"], int(stream["sample_rate"]), stream["channels"]
        pcm = np.frombuffer(subprocess.check_output([
            "ffmpeg", "-v", "error", "-i", str(path), "-map", f"0:{index}",
            "-c:a", "pcm_f32le", "-f", "f32le", "-",
        ]), dtype="<f4").reshape(-1, channels)
        packets = [p for p in metadata["packets"] if p["stream_index"] == index]
        time_base = Fraction(stream["time_base"])
        expected = np.array([int(p["pts"]) + int(p["duration"]) for p in packets[:-1]])
        actual = np.array([int(p["pts"]) for p in packets[1:]])
        gaps = (actual - expected) * float(time_base * rate)
        zeros = runs(np.all(pcm == 0, axis=1))
        # Only ranges inside one complete packet are PCM evidence; a timestamp
        # gap is separately reported as a recorder-delivery gap, never an engine click.
        offset = 0
        internal_zeros = []
        jumps = []
        for packet in packets:
            count = int(packet["size"]) // (4 * channels)
            data = pcm[offset:offset + count]
            for start, length in runs(np.all(data == 0, axis=1)):
                internal_zeros.append({"hostTime": float(int(packet["pts"]) * time_base) + start / rate, "frames": length})
            delta = np.abs(np.diff(data[:, 0]))
            if delta.size:
                at = int(np.argmax(delta))
                jumps.append({"hostTime": float(int(packet["pts"]) * time_base) + at / rate, "amplitude": float(delta[at])})
            offset += count
        result["streams"].append({
            "index": index, "rate": rate, "channels": channels,
            "hostStart": float(stream.get("start_time", 0)), "frames": len(pcm),
            "pcmSeconds": len(pcm) / rate, "packetCount": len(packets),
            "timestampGapCount": int(np.count_nonzero(gaps > 0)),
            "timestampMissingFrames": int(gaps[gaps > 0].sum()),
            "timestampOverlapCount": int(np.count_nonzero(gaps < 0)),
            "maxTimestampGapFrames": int(gaps.max(initial=0)),
            "peak": float(np.max(np.abs(pcm))), "rms": float(np.sqrt(np.mean(pcm ** 2))),
            "exactZeroFrames": sum(length for _, length in zeros),
            "longestZeroFrames": max((length for _, length in zeros), default=0),
            "zeroRunCount": len(zeros),
            "continuousTimestampCoverage": bool(np.all(gaps == 0)),
            "decodedZeroRanges": zeros,
            "internalZeroRanges": internal_zeros,
            "largestInternalJumps": sorted(jumps, key=lambda row: row["amplitude"], reverse=True)[:12],
        })
    return result


if __name__ == "__main__":
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("capture", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    result = analyze(args.capture)
    encoded = json.dumps(result, indent=2)
    if args.output:
        with args.output.open("x") as output:
            output.write(encoded + "\n")
        for stream in result["streams"]:
            print(json.dumps({key: value for key, value in stream.items() if not isinstance(value, list)}))
    else:
        print(encoded)
