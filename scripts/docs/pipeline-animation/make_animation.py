"""Assemble the pipeline animation GIF from the raw frames of render.cs.

Usage: python -I make_animation.py FRAMES_BIN OUTPUT_GIF

The script uses only the standard library. It stores each frame as the changed
rectangle of the previous frame (unchanged pixels use a transparent index), and
it decodes the finished GIF again to prove that every frame is exact.
"""
import struct
import sys
from pathlib import Path

MAGIC = b"PIPEFRM1"
TRANSPARENT = 255
MIN_CODE_SIZE = 8
MAX_BYTES = 10_000_000


def read_header(stream):
    head = stream.read(len(MAGIC) + 12)
    if head[: len(MAGIC)] != MAGIC:
        raise ValueError("not a frames file")
    width, height, count = struct.unpack("<III", head[len(MAGIC):])
    palette = stream.read(768)
    return width, height, count, palette


def frames(path):
    """Yield (width, height, palette, frame bytes) for each frame."""
    with open(path, "rb") as stream:
        width, height, count, palette = read_header(stream)
        for _ in range(count):
            data = stream.read(width * height)
            if len(data) != width * height:
                raise ValueError("truncated frames file")
            yield width, height, palette, data


def lzw_compress(pixels):
    """GIF LZW with 8-bit pixels. Returns the code stream, without sub-blocks."""
    clear = 1 << MIN_CODE_SIZE
    eoi = clear + 1
    out = bytearray()
    acc = 0
    nbits = 0
    size = MIN_CODE_SIZE + 1
    next_code = eoi + 1
    table = {}

    def emit(code):
        nonlocal acc, nbits
        acc |= code << nbits
        nbits += size
        while nbits >= 8:
            out.append(acc & 255)
            acc >>= 8
            nbits -= 8

    emit(clear)
    iterator = iter(pixels)
    prefix = next(iterator)
    for pixel in iterator:
        key = (prefix << 8) | pixel
        code = table.get(key)
        if code is not None:
            prefix = code
            continue
        emit(prefix)
        if next_code < 4096:
            table[key] = next_code
            if next_code == (1 << size) and size < 12:
                size += 1
            next_code += 1
        else:
            emit(clear)
            table.clear()
            size = MIN_CODE_SIZE + 1
            next_code = eoi + 1
        prefix = pixel
    emit(prefix)
    # The decoder adds one more entry when it reads the last code. Mirror its
    # code-size change, so that the end-of-information code has the same width.
    if next_code < 4096 and next_code == (1 << size) and size < 12:
        size += 1
    emit(eoi)
    if nbits:
        out.append(acc & 255)
    return bytes(out)


def lzw_decompress(data):
    """Reference decoder, used only to check the encoder."""
    clear = 1 << MIN_CODE_SIZE
    eoi = clear + 1
    size = MIN_CODE_SIZE + 1
    next_code = eoi + 1
    table = {i: bytes([i]) for i in range(clear)}
    out = bytearray()
    previous = None
    acc = 0
    nbits = 0
    position = 0
    while True:
        while nbits < size:
            if position >= len(data):
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
            table = {i: bytes([i]) for i in range(clear)}
            previous = None
            continue
        if code == eoi:
            return bytes(out)
        if previous is None:
            entry = table[code]
        else:
            if code in table:
                entry = table[code]
            elif code == next_code:
                entry = previous + previous[:1]
            else:
                raise ValueError("invalid code")
            if next_code < 4096:
                table[next_code] = previous + entry[:1]
                next_code += 1
                if next_code == (1 << size) and size < 12:
                    size += 1
        out += entry
        previous = entry


def sub_blocks(data):
    out = bytearray()
    for start in range(0, len(data), 255):
        chunk = data[start:start + 255]
        out.append(len(chunk))
        out += chunk
    out.append(0)
    return bytes(out)


def changed_rectangle(previous, current, width, height):
    """Return (left, top, right, bottom) of the changed pixels, or None."""
    difference = int.from_bytes(previous, "little") ^ int.from_bytes(current, "little")
    if difference == 0:
        return None
    first = ((difference & -difference).bit_length() - 1) // 8
    last = (difference.bit_length() - 1) // 8
    top, bottom = first // width, last // width
    left, right = width, 0
    for y in range(top, bottom + 1):
        row = slice(y * width, (y + 1) * width)
        delta = int.from_bytes(previous[row], "little") ^ int.from_bytes(current[row], "little")
        if delta:
            left = min(left, (((delta & -delta).bit_length() - 1) // 8))
            right = max(right, (delta.bit_length() - 1) // 8)
    return left, top, right + 1, bottom + 1


def frame_delay(index):
    # Centiseconds. Alternating 12 and 13 gives exactly 8 frames per second.
    return 12 if index % 2 == 0 else 13


def assemble(source, target):
    gif = bytearray()
    previous = None
    count = 0
    for index, (width, height, palette, data) in enumerate(frames(source)):
        if index == 0:
            gif += b"GIF89a" + struct.pack("<HHBBB", width, height, 0xF7, 0, 0)
            gif += palette
            gif += b"\x21\xff\x0bNETSCAPE2.0\x03\x01\x00\x00\x00"
            rect = (0, 0, width, height)
            pixels = data
        else:
            rect = changed_rectangle(previous, data, width, height) or (0, 0, 1, 1)
            left, top, right, bottom = rect
            rows = bytearray()
            for y in range(top, bottom):
                old = previous[y * width + left:y * width + right]
                new = data[y * width + left:y * width + right]
                rows += bytes(n if n != o else TRANSPARENT for o, n in zip(old, new))
            pixels = bytes(rows)
        left, top, right, bottom = rect
        gif += b"\x21\xf9\x04\x05" + struct.pack("<H", frame_delay(index)) + bytes([TRANSPARENT, 0])
        gif += b"\x2c" + struct.pack("<HHHHB", left, top, right - left, bottom - top, 0)
        gif += bytes([MIN_CODE_SIZE]) + sub_blocks(lzw_compress(pixels))
        previous = data
        count += 1
    gif += b"\x3b"
    Path(target).write_bytes(gif)
    return len(gif), count


def verify(gif_path, source):
    """Decode the GIF and compare every composited frame with the raw frame."""
    gif = Path(gif_path).read_bytes()
    width, height = struct.unpack("<HH", gif[6:10])
    position = 13 + 768
    canvas = bytearray(width * height)
    expected = frames(source)
    checked = 0
    while gif[position] != 0x3B:
        block = gif[position]
        if block == 0x21:
            position += 2
            while gif[position]:
                position += gif[position] + 1
            position += 1
            continue
        if block != 0x2C:
            raise ValueError(f"unexpected block {block:02x}")
        left, top, w, h, packed = struct.unpack("<HHHHB", gif[position + 1:position + 10])
        if packed:
            raise ValueError("unexpected local color table or interlace")
        position += 10
        if gif[position] != MIN_CODE_SIZE:
            raise ValueError("unexpected minimum code size")
        position += 1
        stream = bytearray()
        while gif[position]:
            length = gif[position]
            stream += gif[position + 1:position + 1 + length]
            position += length + 1
        position += 1
        pixels = lzw_decompress(bytes(stream))
        if len(pixels) != w * h:
            raise ValueError("decoded size mismatch")
        for y in range(h):
            row = pixels[y * w:(y + 1) * w]
            start = (top + y) * width + left
            canvas[start:start + w] = bytes(
                c if n == TRANSPARENT else n for n, c in zip(row, canvas[start:start + w])
            )
        _, _, _, raw = next(expected)
        if bytes(canvas) != raw:
            raise AssertionError(f"frame {checked} differs from the rendered frame")
        checked += 1
    return checked


def main(argv):
    if len(argv) != 3:
        print(__doc__)
        return 2
    source, target = argv[1], argv[2]
    size, count = assemble(source, target)
    checked = verify(target, source)
    if checked != count:
        raise AssertionError(f"verified {checked} of {count} frames")
    if size >= MAX_BYTES:
        raise AssertionError(f"GIF too large: {size:,} bytes")
    print(f"{target}: {size:,} bytes, {count} frames, all verified, infinite loop")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
