"""Receive Ferese's private test EIS connection through stdin using real libei."""
import ctypes as C
import ctypes.util
import json
import os
import select
import time

lib = C.CDLL(ctypes.util.find_library("ei"))


def function(name, result, *arguments):
    value = getattr(lib, name)
    value.restype = result
    value.argtypes = list(arguments)
    return value


ptr = C.c_void_p
new = function("ei_new_receiver", ptr, ptr)
setup = function("ei_setup_backend_fd", C.c_int, ptr, C.c_int)
dispatch = function("ei_dispatch", None, ptr)
next_event = function("ei_get_event", ptr, ptr)
kind = function("ei_event_get_type", C.c_int, ptr)
seat = function("ei_event_get_seat", ptr, ptr)
device = function("ei_event_get_device", ptr, ptr)
unref_event = function("ei_event_unref", ptr, ptr)
unref = function("ei_unref", ptr, ptr)
bind = function("ei_seat_bind_capabilities", None, ptr)
keymap = function("ei_device_keyboard_get_keymap", ptr, ptr)
map_fd = function("ei_keymap_get_fd", C.c_int, ptr)
map_size = function("ei_keymap_get_size", C.c_size_t, ptr)
dx = function("ei_event_pointer_get_dx", C.c_double, ptr)
dy = function("ei_event_pointer_get_dy", C.c_double, ptr)
key = function("ei_event_keyboard_get_key", C.c_uint32, ptr)
pressed = function("ei_event_keyboard_get_key_is_press", C.c_bool, ptr)
stop_x = function("ei_event_scroll_get_stop_x", C.c_bool, ptr)
stop_y = function("ei_event_scroll_get_stop_y", C.c_bool, ptr)
fd = function("ei_get_fd", C.c_int, ptr)
context = new(None)
assert context and setup(context, os.dup(0)) == 0
seen = {"motion": [], "keys": [], "stops": [], "starts": 0, "ends": 0, "keymap": False}
deadline = time.monotonic() + 10
try:
    while time.monotonic() < deadline:
        select.select([fd(context)], [], [], 0.05)
        dispatch(context)
        while event := next_event(context):
            event_type = kind(event)
            if event_type == 2:
                raise AssertionError("EIS disconnected before completing capture")
            if event_type == 3:
                bind(seat(event), C.c_int(1), C.c_int(4), C.c_int(16), C.c_int(32), C.c_void_p())
            elif event_type == 5:
                if mapping := keymap(device(event)):
                    content = os.pread(map_fd(mapping), map_size(mapping), 0)
                    assert content.endswith(b"\0") and b"xkb_keymap" in content
                    seen["keymap"] = True
            elif event_type == 200:
                seen["starts"] += 1
            elif event_type == 201:
                seen["ends"] += 1
            elif event_type == 300:
                seen["motion"].append([dx(event), dy(event)])
            elif event_type == 700:
                seen["keys"].append([key(event), pressed(event)])
            elif event_type == 601:
                seen["stops"].append([stop_x(event), stop_y(event)])
            unref_event(event)
        if seen["ends"] == 2:
            assert seen == {"motion": [[10.0, -5.0], [10.0, -5.0]], "keys": [[30, True], [30, False]], "stops": [[False, True]], "starts": 2, "ends": 2, "keymap": True}, seen
            print(json.dumps(seen), flush=True)
            break
    else:
        raise AssertionError(f"Timed out: {seen}")
finally:
    unref(context)
