#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw


def compare(target: Path, current: Path, output: Path, threshold: int = 0) -> dict:
    left = Image.open(target).convert("RGBA")
    right = Image.open(current).convert("RGBA")
    width = max(left.width, right.width)
    height = max(left.height, right.height)

    if left.size != right.size:
        canvas_left = Image.new("RGBA", (width, height), (255, 0, 255, 255))
        canvas_right = Image.new("RGBA", (width, height), (255, 0, 255, 255))
        canvas_left.paste(left, (0, 0))
        canvas_right.paste(right, (0, 0))
        diff = ImageChops.difference(canvas_left, canvas_right)
        changed = 1.0
        changed_pixels = width * height
        mean_error = 255.0
        bbox = [0, 0, width, height]
    else:
        diff = ImageChops.difference(left, right)
        if threshold:
            alpha = diff.getchannel("A").point(lambda value: 255 if value > threshold else 0)
            mask = ImageChops.lighter(diff.getchannel("R"), diff.getchannel("G"))
            mask = ImageChops.lighter(mask, diff.getchannel("B"))
            mask = ImageChops.multiply(mask, alpha)
        else:
            mask = ImageChops.lighter(diff.getchannel("R"), diff.getchannel("G"))
            mask = ImageChops.lighter(mask, diff.getchannel("B"))
        histogram = mask.histogram()
        changed_pixels = sum(histogram[1:])
        changed = changed_pixels / (width * height)
        mean_error = sum(index * count for index, count in enumerate(histogram)) / (
            (width * height) * 3
        )
        bbox = list(mask.getbbox()) if mask.getbbox() else None

    output.parent.mkdir(parents=True, exist_ok=True)
    heat = Image.new("RGBA", (width, height), (0, 0, 0, 0))
    pixels = heat.load()
    diff_pixels = diff.load()
    for y in range(height):
        for x in range(width):
            delta = max(diff_pixels[x, y][:3])
            if delta:
                pixels[x, y] = (255, 64, 64, min(255, 80 + delta))
    heat.save(output)

    return {
        "target": str(target),
        "current": str(current),
        "targetSize": list(left.size),
        "currentSize": list(right.size),
        "changedPixels": changed_pixels,
        "changedRatio": changed,
        "meanChannelError": mean_error,
        "bbox": bbox,
        "diff": str(output),
        "pass": changed_pixels == 0,
    }


def self_test(target: Path, directory: Path) -> dict:
    source = Image.open(target).convert("RGBA")
    corrupted = source.copy()
    x = min(17, corrupted.width - 1)
    y = min(19, corrupted.height - 1)
    red, green, blue, alpha = corrupted.getpixel((x, y))
    corrupted.putpixel((x, y), (255 - red, 255 - green, 255 - blue, alpha))
    bad = directory / "self-test-bad.png"
    corrupted.save(bad)
    result = compare(target, bad, directory / "self-test-diff.png")
    if result["changedPixels"] < 1:
        raise SystemExit("Self-test failed: a one-pixel change was not detected.")
    return {
        "pass": True,
        "changedPixels": result["changedPixels"],
        "probe": [x, y],
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("target", type=Path)
    parser.add_argument("current", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--threshold", type=int, default=0)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()

    if args.self_test:
        result = self_test(args.target, args.output.parent)
    else:
        result = compare(args.target, args.current, args.output, args.threshold)
    print(json.dumps(result, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
