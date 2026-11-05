/*
 * dgn.h - C interface of the MicroStation DGN reader (dgn.dll, libdgn.so, libdgn.dylib).
 *
 * Reads MicroStation design files: V7 (Intergraph ISFF) and V8 (V8, V8i, CONNECT and later).
 * MicroStation is a trademark of Bentley Systems, Incorporated; this library is not affiliated
 * with or endorsed by Bentley Systems.
 *
 * Strings are UTF-8. Strings returned by dgn_query are freed with dgn_free; all other returned
 * strings are owned by the library. Functions accept NULL handles and then do nothing or fail.
 *
 *     DgnDocument *doc = dgn_open("drawing.dgn");
 *     if (!doc) { fprintf(stderr, "%s\n", dgn_last_error()); return 1; }
 *     char *info = dgn_query(doc, "{\"query\": \"info\"}");
 *     puts(info);
 *     dgn_free(info);
 *     DgnScene *scene = dgn_scene(doc, 0);
 *     for (size_t i = 0; i < dgn_scene_layer_count(scene); i++) {
 *         DgnLayer layer;
 *         dgn_scene_layer(scene, i, &layer);
 *         printf("%s: %zu triangles\n", layer.name, layer.index_count / 3);
 *     }
 *     dgn_scene_free(scene);
 *     dgn_close(doc);
 */
#ifndef DGN_H
#define DGN_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* An open design file. */
typedef struct DgnDocument DgnDocument;

/* The triangle scene of one model: one mesh per level. */
typedef struct DgnScene DgnScene;

/* One layer (level) of a scene. The pointers stay valid until dgn_scene_free. */
typedef struct DgnLayer {
    uint32_t level;          /* V7: level number; V8: level ID */
    const char *name;        /* level name, or "Level n" */
    const float *positions;  /* vertex_count x (x, y, z), metres from the scene origin, Z up */
    const float *normals;    /* vertex_count x (x, y, z), unit length */
    const uint8_t *colors;   /* vertex_count x (r, g, b, a), sRGB */
    size_t vertex_count;
    const uint32_t *indices; /* triangle list */
    size_t index_count;
} DgnLayer;

/* The library version, such as "0.1.0". */
const char *dgn_version(void);

/* Why the last dgn_open, dgn_open_bytes or dgn_scene of this thread failed ("" if it did not). */
const char *dgn_last_error(void);

/* Opens a design file; NULL on failure (see dgn_last_error). */
DgnDocument *dgn_open(const char *path);

/* Reads a design file from memory (the bytes are not kept); NULL on failure. */
DgnDocument *dgn_open_bytes(const uint8_t *data, size_t len);

/*
 * Answers a JSON request with a JSON answer {"ok": true, "result": ...} or
 * {"ok": false, "error": "..."}; free the answer with dgn_free. Requests:
 *   {"query": "info"}                         format, revision, units, levels, models, counts
 *   {"query": "models"}                       models: index, id, name, 2D/3D, hidden, elements
 *   {"query": "levels"}                       levels with name and symbology
 *   {"query": "elements", "model": 0, "offset": 0, "limit": 100}
 *                                             elements with geometry in design coordinates (UOR)
 *   {"query": "shared_cells"}                 shared cell definitions
 *   {"query": "references"}                   reference file attachments
 *   {"query": "design"}                       the whole neutral model
 *   {"query": "drawing", "model": 0}          line work and faces in world metres
 */
char *dgn_query(const DgnDocument *doc, const char *request);

/* Frees an answer of dgn_query. */
void dgn_free(char *s);

/* Closes a document. Scenes built from it stay valid. */
void dgn_close(DgnDocument *doc);

/* The triangle scene of model `model` (0: the default model); NULL when nothing is drawn. */
DgnScene *dgn_scene(const DgnDocument *doc, size_t model);

/* The number of layers of a scene. */
size_t dgn_scene_layer_count(const DgnScene *scene);

/* The scene origin in world metres (three doubles): add it to positions for world coordinates. */
void dgn_scene_origin(const DgnScene *scene, double *xyz);

/* Fills `out` with layer `index`; returns 0 if there is no such layer. */
int dgn_scene_layer(const DgnScene *scene, size_t index, DgnLayer *out);

/* Frees a scene. */
void dgn_scene_free(DgnScene *scene);

#ifdef __cplusplus
}
#endif

#endif /* DGN_H */
