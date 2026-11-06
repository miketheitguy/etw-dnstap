"""Decode the buffer animation GIF and check every frame against the renderer's hashes.

Usage: python -I verify_animation.py OUTPUT_GIF FRAMES_SHA256

The script uses only the standard library. It parses the GIF, decodes every LZW image
with a reference decoder, composites the delta frames onto a canvas, and compares the
SHA-256 of each composited frame with the line that Render.cs wrote for it. It also
checks the size limit, the loop extension and the frame delays. The last line printed
contains "all verified" when every check passed.
"""
import hashlib
import struct
import sys
from pathlib import Path

MIN_CODE_SIZE = 8
TRANSPARENT = 255
MAX_BYTES = 12_000_000
TARGET_BYTES = 9_000_000


def lzw_decompress(data):
    clear = 1 << MIN_CODE_SIZE
    eoi = clear + 1
    size = MIN_CODE_SIZE + 1
    next_code = eoi + 1
    table = [bytes([i]) for i in range(clear)] + [b"", b""]
    out = bytearray()
    previous = None
    acc = 0
    nbits = 0
    position = 0
    length = len(data)
    while True:
        while nbits < size:
            if position >= length:
                raise ValueError("code stream ended without an end code")
            acc |= data[position] << nbits
            position += 1
            nbits += 8
        code = acc & ((1 << size) - 1)
        acc >>= size
        nbits -= size
        if code == clear:
            size = MIN_CODE_SIZE + 1
            next_code = eoi + 1
            del table[next_code:]
            previous = None
            continue
        if code == eoi:
            return bytes(out)
        if previous is None:
            entry = table[code]
        else:
            if code < next_code:
                entry = table[code]
            elif code == next_code:
                entry = previous + previous[:1]
            else:
                raise ValueError("invalid code")
            if next_code < 4096:
                table.append(previous + entry[:1])
                next_code += 1
                if next_code == (1 << size) and size < 12:
                    size += 1
        out += entry
        previous = entry


def verify(gif_path, hashes_path):
    gif = Path(gif_path).read_bytes()
    lines = Path(hashes_path).read_text().split("\n")
    width_h, height_h, count_h, fps = (int(v) for v in lines[0].split())
    expected = [line for line in lines[1:] if line]
    if len(expected) != count_h:
        raise AssertionError(f"hash file lists {len(expected)} frames, header says {count_h}")
    if gif[:6] != b"GIF89a":
        raise ValueError("not a GIF89a file")
    width, height, packed = struct.unpack("<HHB", gif[6:11])
    if (width, height) != (width_h, height_h):
        raise AssertionError("GIF size differs from the rendered size")
    if not packed & 0x80 or (packed & 7) != 7:
        raise ValueError("expected a 256-colour global palette")
    position = 13 + 768
    canvas = bytearray(width * height)
    checked = 0
    looped = False
    delays = []
    pending_delay = None
    while gif[position] != 0x3B:
        block = gif[position]
        if block == 0x21:
            label = gif[position + 1]
            position += 2
            body = bytearray()
            while gif[position]:
                n = gif[position]
                body += gif[position + 1:position + 1 + n]
                position += n + 1
            position += 1
            if label == 0xFF and body[:11] == b"NETSCAPE2.0" and body[11:14] == b"\x01\x00\x00":
                looped = True
            if label == 0xF9:
                flags, delay, transparent = struct.unpack("<BHB", body[:4])
                if not flags & 1 or transparent != TRANSPARENT or (flags >> 2) & 7 != 1:
                    raise ValueError("unexpected graphic control flags")
                pending_delay = delay
            continue
        if block != 0x2C:
            raise ValueError(f"unexpected block {block:02x}")
        left, top, w, h, flags = struct.unpack("<HHHHB", gif[position + 1:position + 10])
        if flags:
            raise ValueError("unexpected local colour table or interlace")
        position += 10
        if gif[position] != MIN_CODE_SIZE:
            raise ValueError("unexpected minimum code size")
        position += 1
        stream = bytearray()
        while gif[position]:
            n = gif[position]
            stream += gif[position + 1:position + 1 + n]
            position += n + 1
        position += 1
        pixels = lzw_decompress(bytes(stream))
        if len(pixels) != w * h:
            raise ValueError("decoded size mismatch")
        for y in range(h):
            row = pixels[y * w:(y + 1) * w]
            start = (top + y) * width + left
            if TRANSPARENT in row:
                old = canvas[start:start + w]
                canvas[start:start + w] = bytes(c if n == TRANSPARENT else n for n, c in zip(row, old))
            else:
                canvas[start:start + w] = row
        digest = hashlib.sha256(canvas).hexdigest()
        if checked >= len(expected) or digest != expected[checked]:
            raise AssertionError(f"frame {checked} differs from the rendered frame")
        delays.append(pending_delay)
        checked += 1
    if checked != count_h:
        raise AssertionError(f"verified {checked} of {count_h} frames")
    if not looped:
        raise AssertionError("no infinite loop extension")
    total = sum(delays)
    if total != round(count_h * 100 / fps):
        raise AssertionError(f"frame delays add up to {total} cs, expected {count_h * 100 // fps}")
    size = len(gif)
    if size > MAX_BYTES:
        raise AssertionError(f"GIF too large: {size:,} bytes")
    note = "" if size <= TARGET_BYTES else " (over the 9 MB target)"
    print(f"{gif_path}: {size:,} bytes, {checked} frames, {width}x{height}, {fps} fps, {total / 100:.1f} s, "
          f"all verified, infinite loop{note}")
    return 0


def main(argv):
    if len(argv) != 3:
        print(__doc__)
        return 2
    return verify(argv[1], argv[2])


if __name__ == "__main__":
    sys.exit(main(sys.argv))
