/* Two overlapping clients with distinct colors for isolated capture tests. */
#define _GNU_SOURCE
#include <stdint.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>
#include <wayland-client.h>
#include "xdg-shell-client-protocol.h"

static struct wl_compositor *compositor;
static struct wl_shm *shm;
static struct xdg_wm_base *wm;
static struct wl_surface *surface;
static struct wl_buffer *buffer;
static int width = 640, height = 480;
static int private_capture_seen;
static int covered;

static void ping(void *data, struct xdg_wm_base *base, uint32_t serial) {
    (void)data;
    xdg_wm_base_pong(base, serial);
}
static const struct xdg_wm_base_listener wm_listener = {.ping = ping};

static void global(void *data, struct wl_registry *registry, uint32_t name,
                   const char *interface, uint32_t version) {
    (void)data; (void)version;
    if (!strcmp(interface, "ferese_window_capture_manager_v1")) private_capture_seen = 1;
    if (!strcmp(interface, "wl_compositor"))
        compositor = wl_registry_bind(registry, name, &wl_compositor_interface, 4);
    else if (!strcmp(interface, "wl_shm"))
        shm = wl_registry_bind(registry, name, &wl_shm_interface, 1);
    else if (!strcmp(interface, "xdg_wm_base")) {
        wm = wl_registry_bind(registry, name, &xdg_wm_base_interface, 1);
        xdg_wm_base_add_listener(wm, &wm_listener, NULL);
    }
}

static void removed(void *data, struct wl_registry *registry, uint32_t name) {
    (void)data; (void)registry; (void)name;
}
static const struct wl_registry_listener registry_listener = {global, removed};

static void configure(void *data, struct xdg_surface *xdg, uint32_t serial) {
    (void)data;
    xdg_surface_ack_configure(xdg, serial);
    xdg_surface_set_window_geometry(xdg, 0, 0, width, height);
    wl_surface_attach(surface, buffer, 0, 0);
    wl_surface_damage(surface, 0, 0, width, height);
    wl_surface_commit(surface);
}
static const struct xdg_surface_listener surface_listener = {.configure = configure};

static void size(void *data, struct xdg_toplevel *top, int32_t w, int32_t h,
                 struct wl_array *states) {
    (void)data; (void)top; (void)w; (void)h; (void)states;
}

static void close_window(void *data, struct xdg_toplevel *top) {
    (void)data; (void)top; exit(0);
}
static const struct xdg_toplevel_listener top_listener = {.configure = size, .close = close_window};

static void make_buffer(void) {
    int fd = memfd_create("floating-size-test", MFD_CLOEXEC);
    size_t bytes = width * height * 4;
    if (fd < 0 || ftruncate(fd, bytes)) exit(3);
    uint32_t *pixels = mmap(NULL, bytes, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (pixels == MAP_FAILED) exit(4);
    for (size_t i = 0; i < (size_t)width * height; i++) {
        pixels[i] = covered ? 0xff00ff00 : (i % width < 80 ? 0x80800000 : (i / width < (size_t)height / 2 ? 0xffff0000 : 0xff0000ff));
    }
    struct wl_shm_pool *pool = wl_shm_create_pool(shm, fd, bytes);
    buffer = wl_shm_pool_create_buffer(pool, 0, width, height, width * 4, WL_SHM_FORMAT_ARGB8888);
    wl_shm_pool_destroy(pool);
    close(fd);
    munmap(pixels, bytes);
}

int main(int argc, char **argv) {
    struct wl_display *display = wl_display_connect(NULL);
    if (!display) return 1;
    struct wl_registry *registry = wl_display_get_registry(display);
    wl_registry_add_listener(registry, &registry_listener, NULL);
    if (wl_display_roundtrip(display) < 0 || !compositor || !shm || !wm) return 2;
    if (private_capture_seen) return 9;
    covered = argc > 1 && !strcmp(argv[1], "cover");
    make_buffer();
    surface = wl_compositor_create_surface(compositor);
    struct xdg_surface *xdg = xdg_wm_base_get_xdg_surface(wm, surface);
    xdg_surface_add_listener(xdg, &surface_listener, NULL);
    struct xdg_toplevel *top = xdg_surface_get_toplevel(xdg);
    xdg_toplevel_add_listener(top, &top_listener, NULL);
    xdg_toplevel_set_app_id(top, "ferese.test.window-capture");
    xdg_toplevel_set_title(top, covered ? "Occluding window" : "Capture target");
    wl_surface_commit(surface);
    if (argc > 1 && !strcmp(argv[1], "resize")) {
        while (1) {
            wl_display_flush(display);
            struct pollfd fds[2] = {{wl_display_get_fd(display), POLLIN, 0}, {STDIN_FILENO, POLLIN, 0}};
            if (poll(fds, 2, -1) < 0) break;
            if (fds[0].revents & POLLIN && wl_display_dispatch(display) < 0) break;
            if (fds[1].revents & POLLIN) {
                char command;
                if (read(STDIN_FILENO, &command, 1) != 1) break;
                if (command == 'r') {
                    width = 480; height = 320;
                    wl_buffer_destroy(buffer);
                    make_buffer();
                    xdg_surface_set_window_geometry(xdg, 0, 0, width, height);
                    wl_surface_attach(surface, buffer, 0, 0);
                    wl_surface_damage(surface, 0, 0, width, height);
                    wl_surface_commit(surface);
                }
            }
        }
    } else {
        while (wl_display_dispatch(display) != -1) {}
    }
    return 0;
}
