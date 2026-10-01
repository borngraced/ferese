#include <assert.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <wayland-client.h>
#include "fractional-scale-client-protocol.h"

static struct wl_compositor *compositor;
static struct wp_fractional_scale_manager_v1 *manager;
static uint32_t version, fractional, fractional_events;
static int32_t integer_scale = 1, transform;
static unsigned scale_events, transform_events;

static void global(void *data, struct wl_registry *registry, uint32_t name,
                   const char *interface, uint32_t advertised) {
    (void)data;
    if (!strcmp(interface, "wl_compositor")) {
        assert(advertised >= 6);
        compositor = wl_registry_bind(registry, name, &wl_compositor_interface, version);
    } else if (!strcmp(interface, "wp_fractional_scale_manager_v1")) {
        manager = wl_registry_bind(registry, name, &wp_fractional_scale_manager_v1_interface, 1);
    }
}

static void removed(void *data, struct wl_registry *registry, uint32_t name) {
    (void)data; (void)registry; (void)name;
}

static void scale(void *data, struct wl_surface *surface, int32_t value) {
    (void)data; (void)surface;
    integer_scale = value;
    scale_events++;
}

static void transformed(void *data, struct wl_surface *surface, uint32_t value) {
    (void)data; (void)surface;
    transform = (int32_t)value;
    transform_events++;
}

static void preferred(void *data, struct wp_fractional_scale_v1 *object, uint32_t value) {
    (void)data; (void)object;
    fractional = value;
    fractional_events++;
}

int main(int argc, char **argv) {
    assert(argc == 2);
    version = (uint32_t)atoi(argv[1]);
    struct wl_display *display = wl_display_connect(NULL);
    assert(display);
    struct wl_registry *registry = wl_display_get_registry(display);
    const struct wl_registry_listener registry_listener = {global, removed};
    wl_registry_add_listener(registry, &registry_listener, NULL);
    assert(wl_display_roundtrip(display) >= 0 && compositor && manager);
    struct wl_surface *surface = wl_compositor_create_surface(compositor);
    const struct wl_surface_listener listener = {
        .preferred_buffer_scale = scale,
        .preferred_buffer_transform = transformed,
    };
    wl_surface_add_listener(surface, &listener, NULL);
    assert(wl_display_roundtrip(display) >= 0);
    // Create the fractional object after the compositor cached its preference.
    struct wp_fractional_scale_v1 *object =
        wp_fractional_scale_manager_v1_get_fractional_scale(manager, surface);
    const struct wp_fractional_scale_v1_listener fractional_listener = {preferred};
    wp_fractional_scale_v1_add_listener(object, &fractional_listener, NULL);
    assert(wl_display_roundtrip(display) >= 0);
    assert(fractional > 0 && fractional_events == 1);
    if (version >= 6) {
        assert(integer_scale == (int32_t)((fractional + 119) / 120));
        assert(scale_events == (integer_scale != 1 ? 1u : 0u));
        assert(transform_events == (transform != 0 ? 1u : 0u));
    } else {
        assert(scale_events == 0 && transform_events == 0);
    }
    for (int i = 0; i < 3; i++) {
        wl_surface_commit(surface);
        assert(wl_display_roundtrip(display) >= 0);
    }
    assert(fractional_events == 1);
    assert(scale_events == (version >= 6 && integer_scale != 1 ? 1u : 0u));
    assert(transform_events == (version >= 6 && transform != 0 ? 1u : 0u));
    wp_fractional_scale_v1_destroy(object);
    wl_surface_destroy(surface);
    wl_display_roundtrip(display);
    wl_display_disconnect(display);
    return 0;
}
