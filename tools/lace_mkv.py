#!/usr/bin/env python3
"""Rewrites an FFmpeg-made MKV so its sound is laced, as mkvmerge writes it.

Each cluster's equal-size sound SimpleBlocks become fixed-laced pairs placed after its pictures,
and an EBML Void fills the bytes saved, so every cluster, cue and seek position stays valid.
Usage: lace_mkv.py in.mkv out.mkv [sound track number, default 2]
"""
import sys


def vint(b, i):
    """(value without its length marker, length) of the EBML number at b[i]."""
    first = b[i]
    n = 1
    while n <= 8 and not first & (0x80 >> (n - 1)):
        n += 1
    value = first & (0xFF >> n)
    for k in range(1, n):
        value = value << 8 | b[i + k]
    return value, n


def element_id(b, i):
    _, n = vint(b, i)
    return int.from_bytes(b[i:i + n], "big"), n


def size_bytes(value):
    """The shortest EBML size encoding for `value`."""
    for n in range(1, 9):
        if value < (1 << (7 * n)) - 1:
            return ((1 << (7 * n)) | value).to_bytes(n, "big")
    raise ValueError(value)


def void(total):
    """An EBML Void element exactly `total` (>= 2) bytes long."""
    for n in range(1, 9):
        if total - 1 - n >= 0 and total - 1 - n < (1 << (7 * n)) - 1:
            return b"\xEC" + ((1 << (7 * n)) | (total - 1 - n)).to_bytes(n, "big") + bytes(total - 1 - n)
    raise ValueError(total)


def children(b, start, end):
    i = start
    while i < end:
        eid, n = element_id(b, i)
        size, m = vint(b, i + n)
        yield eid, i, i + n + m, i + n + m + size
        i = i + n + m + size


def lace_cluster(b, start, end, track):
    """The cluster body with its sound regrouped into laced pairs after the pictures, padded with
    a Void to the same length. Returns it unchanged when the sound can't be paired."""
    keep, sound = bytearray(), []
    for eid, head, body, stop in children(b, start, end):
        t, n = vint(b, body) if eid == 0xA3 else (None, 0)
        if t == track and b[body + n + 2] & 0x06 == 0:
            sound.append((b[body:body + n + 2], b[body + n + 2], b[body + n + 3:stop]))
        else:
            keep += b[head:stop]
    laced = 0
    k = 0
    while k < len(sound):
        prefix, flags, data = sound[k]
        if k + 1 < len(sound) and len(sound[k + 1][2]) == len(data):
            payload = prefix + bytes([flags | 0x04, 1]) + data + sound[k + 1][2]
            laced += 1
            k += 2
        else:
            payload = prefix + bytes([flags]) + data
            k += 1
        keep += b"\xA3" + size_bytes(len(payload)) + payload
    spare = (end - start) - len(keep)
    if laced == 0 or spare < 2:
        return b[start:end], 0
    keep += void(spare)
    return keep, laced


def main():
    src, dst = sys.argv[1], sys.argv[2]
    track = int(sys.argv[3]) if len(sys.argv) > 3 else 2
    b = bytearray(open(src, "rb").read())
    total = 0
    for eid, _, body, stop in children(b, 0, len(b)):
        if eid != 0x18538067:
            continue
        for cid, _, cbody, cstop in children(b, body, min(stop, len(b))):
            if cid == 0x1F43B675:
                body_bytes, laced = lace_cluster(b, cbody, cstop, track)
                b[cbody:cstop] = body_bytes
                total += laced
    open(dst, "wb").write(b)
    print(f"laced {total} pairs of sound frames")


if __name__ == "__main__":
    main()
