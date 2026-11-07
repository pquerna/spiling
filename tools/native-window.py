#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
# Licensed under the Open Software License version 3.0
"""Linux X11 acceptance helper: address only the supplied process's native window."""
import ctypes as c
import os
import struct
import sys
import zlib


def main():
    action, process, *arguments = sys.argv[1:]
    if action not in ("close", "capture") or len(arguments) != (action == "capture"):
        raise ValueError("usage: native-window.py close PID | capture PID NEW.png")
    pid = int(process)
    x = c.CDLL("libX11.so.6")
    window = c.c_ulong
    display = c.c_void_p
    x.XOpenDisplay.argtypes, x.XOpenDisplay.restype = [c.c_char_p], display
    x.XDefaultRootWindow.argtypes, x.XDefaultRootWindow.restype = [display], window
    x.XInternAtom.argtypes, x.XInternAtom.restype = [display, c.c_char_p, c.c_int], window
    x.XQueryTree.argtypes = [display, window, c.POINTER(window), c.POINTER(window), c.POINTER(c.POINTER(window)), c.POINTER(c.c_uint)]
    x.XGetWindowProperty.argtypes = [display, window, window, c.c_long, c.c_long, c.c_int, window, c.POINTER(window), c.POINTER(c.c_int), c.POINTER(c.c_ulong), c.POINTER(c.c_ulong), c.POINTER(c.c_void_p)]
    x.XFree.argtypes = [c.c_void_p]
    x.XFlush.argtypes = [display]
    x.XCloseDisplay.argtypes = [display]
    d = x.XOpenDisplay(os.fsencode(os.environ.get("DISPLAY", "")))
    if not d:
        raise RuntimeError("An accessible X11 DISPLAY is required")
    try:
        pid_atom = x.XInternAtom(d, b"_NET_WM_PID", 0)
        def find(w):
            kind, fmt, count, after, data = window(), c.c_int(), c.c_ulong(), c.c_ulong(), c.c_void_p()
            status = x.XGetWindowProperty(d, w, pid_atom, 0, 1, 0, 0, c.byref(kind), c.byref(fmt), c.byref(count), c.byref(after), c.byref(data))
            matched = status == 0 and fmt.value == 32 and count.value == 1 and data.value and c.cast(data, c.POINTER(c.c_ulong))[0] == pid
            if data.value:
                x.XFree(data)
            if matched:
                return w
            root, parent, children, n = window(), window(), c.POINTER(window)(), c.c_uint()
            if not x.XQueryTree(d, w, c.byref(root), c.byref(parent), c.byref(children), c.byref(n)):
                return None
            try:
                for index in range(n.value):
                    found = find(children[index])
                    if found:
                        return found
            finally:
                if children:
                    x.XFree(children)
            return None
        w = find(x.XDefaultRootWindow(d))
        if not w:
            raise RuntimeError(f"No X11 window owned by process {pid}")
        if action == "close":
            class Data(c.Union):
                _fields_ = [("bytes", c.c_char * 20), ("shorts", c.c_short * 10), ("longs", c.c_long * 5)]
            class ClientMessage(c.Structure):
                _fields_ = [("type", c.c_int), ("serial", c.c_ulong), ("send_event", c.c_int), ("display", display), ("window", window), ("message_type", window), ("format", c.c_int), ("data", Data)]
            class Event(c.Union):
                _fields_ = [("message", ClientMessage), ("padding", c.c_long * 24)]
            event = Event()
            event.message = ClientMessage(33, 0, 1, d, w, x.XInternAtom(d, b"WM_PROTOCOLS", 0), 32)
            event.message.data.longs[0] = x.XInternAtom(d, b"WM_DELETE_WINDOW", 0)
            x.XSendEvent.argtypes = [display, window, c.c_int, c.c_long, c.POINTER(Event)]
            if not x.XSendEvent(d, w, 0, 0, c.byref(event)):
                raise RuntimeError("Native close request failed")
            x.XFlush(d)
        else:
            class Image(c.Structure):
                _fields_ = [(name, c.c_int) for name in ("width", "height", "xoffset", "format")] + [("data", c.c_void_p)] + [(name, c.c_int) for name in ("byte_order", "bitmap_unit", "bitmap_bit_order", "bitmap_pad", "depth", "bytes_per_line", "bits_per_pixel")]
            x.XGetGeometry.argtypes = [display, window, c.POINTER(window), c.POINTER(c.c_int), c.POINTER(c.c_int), c.POINTER(c.c_uint), c.POINTER(c.c_uint), c.POINTER(c.c_uint), c.POINTER(c.c_uint)]
            root, left, top, width, height, border, depth = window(), c.c_int(), c.c_int(), c.c_uint(), c.c_uint(), c.c_uint(), c.c_uint()
            if not x.XGetGeometry(d, w, c.byref(root), c.byref(left), c.byref(top), c.byref(width), c.byref(height), c.byref(border), c.byref(depth)):
                raise RuntimeError("Native window geometry unavailable")
            if not 0 < width.value * height.value <= 16_000_000:
                raise ValueError("Capture exceeds 16 megapixels")
            x.XGetImage.argtypes, x.XGetImage.restype = [display, window, c.c_int, c.c_int, c.c_uint, c.c_uint, c.c_ulong, c.c_int], c.POINTER(Image)
            pointer = x.XGetImage(d, w, 0, 0, width.value, height.value, c.c_ulong(-1), 2)
            if not pointer:
                raise RuntimeError("Native window readback failed")
            try:
                image = pointer.contents
                if image.bits_per_pixel != 32 or image.byte_order != 0:
                    raise ValueError("Capture requires little-endian 32-bit X11 pixels")
                raw = c.string_at(image.data, image.bytes_per_line * image.height)
                rows = bytearray()
                for row in range(image.height):
                    source = raw[row * image.bytes_per_line:row * image.bytes_per_line + image.width * 4]
                    rgba = bytearray(len(source))
                    rgba[0::4], rgba[1::4], rgba[2::4], rgba[3::4] = source[2::4], source[1::4], source[0::4], b"\xff" * image.width
                    rows.extend(b"\x00" + rgba)
                def chunk(kind, content):
                    return struct.pack(">I", len(content)) + kind + content + struct.pack(">I", zlib.crc32(kind + content))
                with open(arguments[0], "xb") as output:
                    output.write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", image.width, image.height, 8, 6, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))
            finally:
                x.XDestroyImage.argtypes = [c.POINTER(Image)]
                x.XDestroyImage(pointer)
    finally:
        x.XCloseDisplay(d)


if __name__ == "__main__":
    main()
