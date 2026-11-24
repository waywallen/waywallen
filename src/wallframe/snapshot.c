#define _GNU_SOURCE
#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <GLES2/gl2.h>
#include <GLES2/gl2ext.h>
#include <dlfcn.h>
#include <errno.h>
#include <poll.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/sysmacros.h>

static pthread_mutex_t capture_lock = PTHREAD_MUTEX_INITIALIZER;

/* A private surfaceless context keeps capture independent of the desktop backend. */
int ww_snapshot_read(uint32_t gpu_major, uint32_t gpu_minor,
                     uint32_t width, uint32_t height, uint32_t fourcc,
                     uint64_t modifier, uint32_t planes, const int *fds,
                     const uint32_t *offsets, const uint32_t *strides,
                     int fence, unsigned char *rgba, char *error, size_t error_len)
{
    pthread_mutex_lock(&capture_lock);
    int result = -1;
    void *library = NULL;
    EGLDisplay display = EGL_NO_DISPLAY;
    EGLContext context = EGL_NO_CONTEXT;
    EGLImageKHR image = EGL_NO_IMAGE_KHR;
    GLuint texture = 0, framebuffer = 0;
    PFNEGLTERMINATEPROC terminate = NULL;
    PFNEGLDESTROYCONTEXTPROC destroy_context = NULL;
    PFNEGLMAKECURRENTPROC make_current = NULL;
    PFNEGLDESTROYIMAGEKHRPROC destroy_image = NULL;
    PFNGLDELETETEXTURESPROC delete_textures = NULL;
    PFNGLDELETEFRAMEBUFFERSPROC delete_framebuffers = NULL;
    if (error_len) error[0] = 0;
#define FAIL(message) do { snprintf(error, error_len, "%s", message); goto done; } while (0)
#define LOAD(type, name) \
    type name = (type)get_proc(#name); \
    if (!name) FAIL("snapshot: missing " #name)
    struct pollfd wait = {.fd = fence, .events = POLLIN};
    int ready;
    do { ready = poll(&wait, 1, 5000); } while (ready < 0 && errno == EINTR);
    if (ready <= 0 || !(wait.revents & POLLIN)) FAIL("snapshot: acquire fence did not signal");
    if (planes == 0 || planes > 4) FAIL("snapshot: invalid plane count");
    library = dlopen("libEGL.so.1", RTLD_NOW | RTLD_LOCAL);
    if (!library) FAIL("snapshot: libEGL.so.1 is unavailable");
    PFNEGLGETPROCADDRESSPROC get_proc = (PFNEGLGETPROCADDRESSPROC)dlsym(library, "eglGetProcAddress");
    if (!get_proc) FAIL("snapshot: eglGetProcAddress is unavailable");
    LOAD(PFNEGLGETPLATFORMDISPLAYEXTPROC, eglGetPlatformDisplayEXT);
    LOAD(PFNEGLQUERYDEVICESEXTPROC, eglQueryDevicesEXT);
    LOAD(PFNEGLQUERYDEVICESTRINGEXTPROC, eglQueryDeviceStringEXT);
    LOAD(PFNEGLINITIALIZEPROC, eglInitialize);
    LOAD(PFNEGLBINDAPIPROC, eglBindAPI);
    LOAD(PFNEGLCHOOSECONFIGPROC, eglChooseConfig);
    LOAD(PFNEGLCREATECONTEXTPROC, eglCreateContext);
    LOAD(PFNEGLCREATEIMAGEKHRPROC, eglCreateImageKHR);
    terminate = (PFNEGLTERMINATEPROC)get_proc("eglTerminate");
    destroy_context = (PFNEGLDESTROYCONTEXTPROC)get_proc("eglDestroyContext");
    make_current = (PFNEGLMAKECURRENTPROC)get_proc("eglMakeCurrent");
    destroy_image = (PFNEGLDESTROYIMAGEKHRPROC)get_proc("eglDestroyImageKHR");
    delete_textures = (PFNGLDELETETEXTURESPROC)get_proc("glDeleteTextures");
    delete_framebuffers = (PFNGLDELETEFRAMEBUFFERSPROC)get_proc("glDeleteFramebuffers");
    if (!terminate || !destroy_context || !make_current || !destroy_image ||
        !delete_textures || !delete_framebuffers) FAIL("snapshot: incomplete EGL/GLES API");

    EGLDeviceEXT devices[32];
    EGLint count = 0;
    if (!eglQueryDevicesEXT(32, devices, &count)) FAIL("snapshot: cannot enumerate EGL devices");
    for (EGLint i = 0; i < count; ++i) {
        const char *path = eglQueryDeviceStringEXT(devices[i], EGL_DRM_RENDER_NODE_FILE_EXT);
        struct stat info;
        if (gpu_major && (!path || stat(path, &info) ||
            major(info.st_rdev) != gpu_major || minor(info.st_rdev) != gpu_minor)) continue;
        EGLDisplay candidate = eglGetPlatformDisplayEXT(EGL_PLATFORM_DEVICE_EXT, devices[i], NULL);
        if (candidate != EGL_NO_DISPLAY && eglInitialize(candidate, NULL, NULL)) {
            display = candidate;
            break;
        }
    }
    if (display == EGL_NO_DISPLAY) FAIL("snapshot: renderer GPU has no usable EGL display");
    if (!eglBindAPI(EGL_OPENGL_ES_API)) FAIL("snapshot: cannot bind GLES API");
    const EGLint config_attrs[] = {EGL_SURFACE_TYPE, EGL_PBUFFER_BIT,
        EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT, EGL_NONE};
    EGLConfig config;
    if (!eglChooseConfig(display, config_attrs, &config, 1, &count) || count != 1)
        FAIL("snapshot: no GLES configuration");
    const EGLint context_attrs[] = {EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE};
    context = eglCreateContext(display, config, EGL_NO_CONTEXT, context_attrs);
    if (context == EGL_NO_CONTEXT || !make_current(display, EGL_NO_SURFACE, EGL_NO_SURFACE, context))
        FAIL("snapshot: cannot create surfaceless GLES context");

    const EGLint fd_keys[] = {EGL_DMA_BUF_PLANE0_FD_EXT, EGL_DMA_BUF_PLANE1_FD_EXT,
        EGL_DMA_BUF_PLANE2_FD_EXT, EGL_DMA_BUF_PLANE3_FD_EXT};
    const EGLint offset_keys[] = {EGL_DMA_BUF_PLANE0_OFFSET_EXT, EGL_DMA_BUF_PLANE1_OFFSET_EXT,
        EGL_DMA_BUF_PLANE2_OFFSET_EXT, EGL_DMA_BUF_PLANE3_OFFSET_EXT};
    const EGLint stride_keys[] = {EGL_DMA_BUF_PLANE0_PITCH_EXT, EGL_DMA_BUF_PLANE1_PITCH_EXT,
        EGL_DMA_BUF_PLANE2_PITCH_EXT, EGL_DMA_BUF_PLANE3_PITCH_EXT};
    const EGLint lo_keys[] = {EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT, EGL_DMA_BUF_PLANE1_MODIFIER_LO_EXT,
        EGL_DMA_BUF_PLANE2_MODIFIER_LO_EXT, EGL_DMA_BUF_PLANE3_MODIFIER_LO_EXT};
    const EGLint hi_keys[] = {EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT, EGL_DMA_BUF_PLANE1_MODIFIER_HI_EXT,
        EGL_DMA_BUF_PLANE2_MODIFIER_HI_EXT, EGL_DMA_BUF_PLANE3_MODIFIER_HI_EXT};
    EGLint attrs[48] = {EGL_WIDTH, (EGLint)width, EGL_HEIGHT, (EGLint)height,
        EGL_LINUX_DRM_FOURCC_EXT, (EGLint)fourcc};
    int n = 6;
    for (uint32_t p = 0; p < planes; ++p) {
        if (offsets[p] > INT32_MAX || strides[p] > INT32_MAX) FAIL("snapshot: invalid plane layout");
        attrs[n++] = fd_keys[p]; attrs[n++] = fds[p];
        attrs[n++] = offset_keys[p]; attrs[n++] = (EGLint)offsets[p];
        attrs[n++] = stride_keys[p]; attrs[n++] = (EGLint)strides[p];
        if (modifier != UINT64_C(0x00ffffffffffffff)) {
            attrs[n++] = lo_keys[p]; attrs[n++] = (EGLint)(modifier & UINT32_MAX);
            attrs[n++] = hi_keys[p]; attrs[n++] = (EGLint)(modifier >> 32);
        }
    }
    attrs[n] = EGL_NONE;
    image = eglCreateImageKHR(display, EGL_NO_CONTEXT, EGL_LINUX_DMA_BUF_EXT, NULL, attrs);
    if (image == EGL_NO_IMAGE_KHR) FAIL("snapshot: cannot import renderer DMA-BUF");
    LOAD(PFNGLGENTEXTURESPROC, glGenTextures);
    LOAD(PFNGLBINDTEXTUREPROC, glBindTexture);
    LOAD(PFNGLEGLIMAGETARGETTEXTURE2DOESPROC, glEGLImageTargetTexture2DOES);
    LOAD(PFNGLGENFRAMEBUFFERSPROC, glGenFramebuffers);
    LOAD(PFNGLBINDFRAMEBUFFERPROC, glBindFramebuffer);
    LOAD(PFNGLFRAMEBUFFERTEXTURE2DPROC, glFramebufferTexture2D);
    LOAD(PFNGLCHECKFRAMEBUFFERSTATUSPROC, glCheckFramebufferStatus);
    LOAD(PFNGLREADPIXELSPROC, glReadPixels);
    LOAD(PFNGLGETERRORPROC, glGetError);
    glGenTextures(1, &texture);
    glBindTexture(GL_TEXTURE_2D, texture);
    glEGLImageTargetTexture2DOES(GL_TEXTURE_2D, image);
    glGenFramebuffers(1, &framebuffer);
    glBindFramebuffer(GL_FRAMEBUFFER, framebuffer);
    glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, texture, 0);
    if (glCheckFramebufferStatus(GL_FRAMEBUFFER) != GL_FRAMEBUFFER_COMPLETE)
        FAIL("snapshot: imported DMA-BUF is not readable");
    glReadPixels(0, 0, (GLsizei)width, (GLsizei)height, GL_RGBA, GL_UNSIGNED_BYTE, rgba);
    if (glGetError() != GL_NO_ERROR) FAIL("snapshot: GLES readback failed");
    result = 0;
done:
    if (framebuffer) delete_framebuffers(1, &framebuffer);
    if (texture) delete_textures(1, &texture);
    if (image != EGL_NO_IMAGE_KHR) destroy_image(display, image);
    if (context != EGL_NO_CONTEXT) {
        make_current(display, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
        destroy_context(display, context);
    }
    if (display != EGL_NO_DISPLAY) terminate(display);
    if (library) dlclose(library);
    pthread_mutex_unlock(&capture_lock);
    return result;
}
