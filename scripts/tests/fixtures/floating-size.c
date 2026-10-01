/* A floating client which chooses a larger window than the configure suggests. */
#define _GNU_SOURCE
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>
#include <wayland-client.h>
#include "xdg-shell-client-protocol.h"
#ifdef FERESE_TEST_DECORATION
#include "xdg-decoration-client-protocol.h"
static struct zxdg_decoration_manager_v1 *decoration_manager;
#endif

static struct wl_compositor *compositor;
static struct wl_shm *shm;
static struct xdg_wm_base *wm;
static struct wl_surface *surface;
static struct wl_buffer *buffer;
static int width = 640, height = 480;
static int honor_configure;
static int buffer_width, buffer_height;

static void make_buffer(void) {
    if (buffer && buffer_width == width && buffer_height == height) return;
    int fd = memfd_create("floating-size-test", MFD_CLOEXEC);
    size_t bytes = (size_t)width * height * 4;
    if (fd < 0 || ftruncate(fd, bytes)) exit(3);
    uint32_t *pixels = mmap(NULL, bytes, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (pixels == MAP_FAILED) exit(4);
    for (size_t i = 0; i < (size_t)width * height; i++) pixels[i] = 0xff12ed34;
    struct wl_shm_pool *pool = wl_shm_create_pool(shm, fd, bytes);
    if (buffer) wl_buffer_destroy(buffer);
    buffer = wl_shm_pool_create_buffer(pool, 0, width, height, width * 4, WL_SHM_FORMAT_XRGB8888);
    wl_shm_pool_destroy(pool);
    munmap(pixels, bytes);
    close(fd);
    buffer_width = width; buffer_height = height;
}
static void ping(void *data, struct xdg_wm_base *base, uint32_t serial) {
    (void)data;
    xdg_wm_base_pong(base, serial);
}
static const struct xdg_wm_base_listener wm_listener = {.ping = ping};
static void global(void *data, struct wl_registry *registry, uint32_t name,
                   const char *interface, uint32_t version) {
    (void)data; (void)version;
    if (!strcmp(interface, "wl_compositor"))
        compositor = wl_registry_bind(registry, name, &wl_compositor_interface, 4);
    else if (!strcmp(interface, "wl_shm"))
        shm = wl_registry_bind(registry, name, &wl_shm_interface, 1);
    else if (!strcmp(interface, "xdg_wm_base")) {
        wm = wl_registry_bind(registry, name, &xdg_wm_base_interface, 1);
        xdg_wm_base_add_listener(wm, &wm_listener, NULL);
    }
#ifdef FERESE_TEST_DECORATION
    else if (!strcmp(interface, "zxdg_decoration_manager_v1"))
        decoration_manager = wl_registry_bind(registry, name, &zxdg_decoration_manager_v1_interface, 1);
#endif
}
static void removed(void *data, struct wl_registry *registry, uint32_t name) {
    (void)data; (void)registry; (void)name;
}
static const struct wl_registry_listener registry_listener = {global, removed};
static void configure(void *data, struct xdg_surface *xdg, uint32_t serial) {
    (void)data;
    xdg_surface_ack_configure(xdg, serial);
    make_buffer();
    printf("buffer %d %d\n", width, height);
    fflush(stdout);
    xdg_surface_set_window_geometry(xdg, 0, 0, width, height);
    wl_surface_attach(surface, buffer, 0, 0);
    wl_surface_damage(surface, 0, 0, width, height);
    wl_surface_commit(surface);
}
static const struct xdg_surface_listener surface_listener = {.configure = configure};
static void size(void *data, struct xdg_toplevel *top, int32_t w, int32_t h,
                 struct wl_array *states) {
    (void)data; (void)top; (void)states;
    printf("configure %d %d\n", w, h);
    fflush(stdout);
    if (honor_configure) {
        if (w > 0) width = w;
        if (h > 0) height = h;
    }
}
static void close_window(void *data, struct xdg_toplevel *top) {
    (void)data; (void)top; exit(0);
}
static const struct xdg_toplevel_listener top_listener = {.configure = size, .close = close_window};
#ifdef FERESE_TEST_DECORATION
static void decoration_configure(void *data, struct zxdg_toplevel_decoration_v1 *decoration, uint32_t mode) {
    (void)data; (void)decoration;
    printf("decoration %u\n", mode);
    fflush(stdout);
}
static const struct zxdg_toplevel_decoration_v1_listener decoration_listener = {.configure = decoration_configure};
#endif
int main(int argc, char **argv) {
    if (argc >= 4) { width = atoi(argv[2]); height = atoi(argv[3]); }
    honor_configure = argc >= 5;
    if (width < 1 || height < 1 || width > 2048 || height > 2048) return 5;
    struct wl_display *display = wl_display_connect(NULL);
    if (!display) return 1;
    struct wl_registry *registry = wl_display_get_registry(display);
    wl_registry_add_listener(registry, &registry_listener, NULL);
    if (wl_display_roundtrip(display) < 0 || !compositor || !shm || !wm) return 2;
    surface = wl_compositor_create_surface(compositor);
    struct xdg_surface *xdg = xdg_wm_base_get_xdg_surface(wm, surface);
    xdg_surface_add_listener(xdg, &surface_listener, NULL);
    struct xdg_toplevel *top = xdg_surface_get_toplevel(xdg);
    xdg_toplevel_add_listener(top, &top_listener, NULL);
#ifdef FERESE_TEST_DECORATION
    if (argc >= 6 && !strcmp(argv[5], "decorated")) {
        if (!decoration_manager) return 6;
        struct zxdg_toplevel_decoration_v1 *decoration =
            zxdg_decoration_manager_v1_get_toplevel_decoration(decoration_manager, top);
        zxdg_toplevel_decoration_v1_add_listener(decoration, &decoration_listener, NULL);
        zxdg_toplevel_decoration_v1_set_mode(decoration,
            argc >= 7 && !strcmp(argv[6], "server")
                ? ZXDG_TOPLEVEL_DECORATION_V1_MODE_SERVER_SIDE
                : ZXDG_TOPLEVEL_DECORATION_V1_MODE_CLIENT_SIDE);
    }
#endif
    xdg_toplevel_set_app_id(top, argc >= 2 ? argv[1] : "ferese.test.floating-size");
    xdg_toplevel_set_title(top, "Floating size regression");
    wl_surface_commit(surface);
    while (wl_display_dispatch(display) != -1) {}
    return 0;
}
