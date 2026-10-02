#define _GNU_SOURCE
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>
#include <wayland-client.h>
#include "xdg-shell-client-protocol.h"
#include "ferese-effects-client-protocol.h"

static struct wl_compositor *compositor;
static struct wl_shm *shm;
static struct xdg_wm_base *wm;
static struct ferese_effects_manager_v1 *effects;

static void ping(void *data, struct xdg_wm_base *object, uint32_t serial) {
    (void)data;
    xdg_wm_base_pong(object, serial);
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
    } else if (!strcmp(interface, "ferese_effects_manager_v1"))
        effects = wl_registry_bind(registry, name, &ferese_effects_manager_v1_interface, 3);
}
static void removed(void *data, struct wl_registry *registry, uint32_t name) {
    (void)data; (void)registry; (void)name;
}
static const struct wl_registry_listener registry_listener = {global, removed};

static void configure(void *data, struct xdg_surface *surface, uint32_t serial) {
    xdg_surface_ack_configure(surface, serial);
    *(int *)data = 1;
}
static const struct xdg_surface_listener surface_listener = {.configure = configure};

static void frame_done(void *data, struct wl_callback *callback, uint32_t time) {
    (void)time;
    *(int *)data = 1;
    wl_callback_destroy(callback);
}
static const struct wl_callback_listener frame_listener = {.done = frame_done};

static void present(struct wl_display *display, struct wl_surface *surface,
                    struct wl_buffer *buffer, const char *phase) {
    int done = 0;
    struct wl_callback *callback = wl_surface_frame(surface);
    wl_callback_add_listener(callback, &frame_listener, &done);
    wl_surface_attach(surface, buffer, 0, 0);
    wl_surface_damage(surface, 0, 0, 320, 240);
    wl_surface_commit(surface);
    while (!done)
        assert(wl_display_dispatch(display) >= 0);
    puts(phase);
    fflush(stdout);
}

int main(void) {
    struct wl_display *display = wl_display_connect(NULL);
    assert(display);
    struct wl_registry *registry = wl_display_get_registry(display);
    wl_registry_add_listener(registry, &registry_listener, NULL);
    assert(wl_display_roundtrip(display) >= 0);
    assert(compositor && shm && wm && effects);

    int fd = memfd_create("blurred-popup", MFD_CLOEXEC);
    assert(fd >= 0 && ftruncate(fd, 320 * 240 * 4) == 0);
    uint32_t *pixels = mmap(NULL, 320 * 240 * 4, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    assert(pixels != MAP_FAILED);
    for (size_t i = 0; i < 320 * 240; i++)
        pixels[i] = 0x20101010;
    struct wl_shm_pool *pool = wl_shm_create_pool(shm, fd, 320 * 240 * 4);
    struct wl_buffer *buffer = wl_shm_pool_create_buffer(pool, 0, 320, 240, 320 * 4, WL_SHM_FORMAT_ARGB8888);
    wl_shm_pool_destroy(pool);
    close(fd);

    int configured = 0;
    struct wl_surface *parent = wl_compositor_create_surface(compositor);
    struct xdg_surface *parent_xdg = xdg_wm_base_get_xdg_surface(wm, parent);
    xdg_surface_add_listener(parent_xdg, &surface_listener, &configured);
    struct xdg_toplevel *top = xdg_surface_get_toplevel(parent_xdg);
    xdg_toplevel_set_app_id(top, "ferese.test.blurred-popup");
    struct ferese_surface_effects_v1 *parent_effects = ferese_effects_manager_v1_get_surface_effects(effects, parent);
    ferese_surface_effects_v1_set_role(parent_effects, FERESE_SURFACE_EFFECTS_V1_ROLE_PANEL);
    wl_surface_commit(parent);
    while (!configured)
        assert(wl_display_dispatch(display) >= 0);
    present(display, parent, buffer, "parent");

    for (int cycle = 0; cycle < 3; cycle++) {
        configured = 0;
        struct wl_surface *surface = wl_compositor_create_surface(compositor);
        struct xdg_surface *xdg = xdg_wm_base_get_xdg_surface(wm, surface);
        xdg_surface_add_listener(xdg, &surface_listener, &configured);
        struct xdg_positioner *positioner = xdg_wm_base_create_positioner(wm);
        xdg_positioner_set_size(positioner, 320, 240);
        xdg_positioner_set_anchor_rect(positioner, 20, 20, 10, 10);
        struct xdg_popup *popup = xdg_surface_get_popup(xdg, parent_xdg, positioner);
        xdg_positioner_destroy(positioner);
        struct ferese_surface_effects_v1 *material = ferese_effects_manager_v1_get_surface_effects(effects, surface);
        ferese_surface_effects_v1_set_role(material, FERESE_SURFACE_EFFECTS_V1_ROLE_POPOVER);
        wl_surface_commit(surface);
        while (!configured)
            assert(wl_display_dispatch(display) >= 0);
        present(display, surface, buffer, "open");
        for (int step = 0; step < 3; step++) {
            ferese_surface_effects_v1_set_opacity(material, 400 + step * 200);
            pixels[step] = 0x30202020;
            present(display, surface, buffer, "update");
        }
        ferese_surface_effects_v1_destroy(material);
        xdg_popup_destroy(popup);
        xdg_surface_destroy(xdg);
        wl_surface_destroy(surface);
        present(display, parent, buffer, "close");
    }
    ferese_surface_effects_v1_destroy(parent_effects);
    xdg_toplevel_destroy(top);
    xdg_surface_destroy(parent_xdg);
    wl_surface_destroy(parent);
    wl_buffer_destroy(buffer);
    munmap(pixels, 320 * 240 * 4);
    wl_display_disconnect(display);
    return 0;
}
