#define _GNU_SOURCE
#include <stdint.h>
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

static void ping(void *data, struct xdg_wm_base *base, uint32_t serial) {
    (void)data;
    xdg_wm_base_pong(base, serial);
}

static const struct xdg_wm_base_listener wm_listener = {.ping = ping};

static void global(void *data, struct wl_registry *registry, uint32_t name,
                   const char *interface, uint32_t version) {
    (void)data;
    (void)version;
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
    (void)data;
    (void)registry;
    (void)name;
}

static const struct wl_registry_listener registry_listener = {global, removed};

static void configure(void *data, struct xdg_surface *xdg, uint32_t serial) {
    (void)data;
    xdg_surface_ack_configure(xdg, serial);
}

static const struct xdg_surface_listener surface_listener = {.configure = configure};

static void size(void *data, struct xdg_toplevel *top, int32_t width, int32_t height,
                 struct wl_array *states) {
    (void)data;
    (void)top;
    (void)width;
    (void)height;
    (void)states;
}

static void close_window(void *data, struct xdg_toplevel *top) {
    (void)data;
    (void)top;
    exit(0);
}

static const struct xdg_toplevel_listener top_listener = {
    .configure = size,
    .close = close_window,
};

static void phase(struct wl_display *display, const char *name) {
    if (wl_display_roundtrip(display) < 0)
        exit(5);
    puts(name);
    fflush(stdout);
    if (getchar() == EOF)
        exit(6);
}

int main(void) {
    struct wl_display *display = wl_display_connect(NULL);
    if (!display)
        return 1;
    struct wl_registry *registry = wl_display_get_registry(display);
    wl_registry_add_listener(registry, &registry_listener, NULL);
    if (wl_display_roundtrip(display) < 0 || !compositor || !shm || !wm)
        return 2;

    int fd = memfd_create("unmapped-toplevel", MFD_CLOEXEC);
    if (fd < 0 || ftruncate(fd, 64 * 64 * 4))
        return 3;
    uint32_t *pixels = mmap(NULL, 64 * 64 * 4, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (pixels == MAP_FAILED)
        return 4;
    for (size_t i = 0; i < 64 * 64; i++)
        pixels[i] = 0xff3182ce;
    struct wl_shm_pool *pool = wl_shm_create_pool(shm, fd, 64 * 64 * 4);
    struct wl_buffer *buffer = wl_shm_pool_create_buffer(
        pool, 0, 64, 64, 64 * 4, WL_SHM_FORMAT_XRGB8888);
    wl_shm_pool_destroy(pool);
    close(fd);

    surface = wl_compositor_create_surface(compositor);
    struct xdg_surface *xdg = xdg_wm_base_get_xdg_surface(wm, surface);
    xdg_surface_add_listener(xdg, &surface_listener, NULL);
    struct xdg_toplevel *top = xdg_surface_get_toplevel(xdg);
    xdg_toplevel_add_listener(top, &top_listener, NULL);
    xdg_toplevel_set_app_id(top, "ferese.test.unmapped");
    xdg_toplevel_set_title(top, "Buffer lifecycle test");

    wl_surface_commit(surface);
    phase(display, "initial-empty");

    xdg_surface_set_window_geometry(xdg, 0, 0, 64, 64);
    wl_surface_attach(surface, buffer, 0, 0);
    wl_surface_damage(surface, 0, 0, 64, 64);
    wl_surface_commit(surface);
    phase(display, "visible");

    wl_surface_attach(surface, NULL, 0, 0);
    wl_surface_commit(surface);
    phase(display, "detached");

    wl_surface_attach(surface, buffer, 0, 0);
    wl_surface_damage(surface, 0, 0, 64, 64);
    wl_surface_commit(surface);
    phase(display, "remapped");
    return 0;
}
