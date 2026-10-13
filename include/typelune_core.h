/*
 * typelune_core.h — C ABI of the Typelune Rust core (libtypelune_core.a), ABI version 2.
 * Used by the native SwiftUI app. src/ffi.rs is the authoritative contract.
 *
 * Ownership:
 *   - tpl_core_open returns an owned handle, or NULL on failure (including an empty or
 *     non-UTF-8 data_dir); release it with tpl_core_close exactly once.
 *   - tpl_core_call returns an owned NUL-terminated UTF-8 JSON string; release it with
 *     tpl_string_free. Replies: {"ok":true,"result":...} or {"ok":false,"error":"..."}.
 * Safety:
 *   - Panics inside Rust are caught and reported as {"ok":false,...}; they never unwind.
 *   - Opening the core never starts system-wide input monitoring.
 *   - With data_dir == NULL, the default folder is used and a pre-0.3 library is copied
 *     once (never moved or overwritten); see src/migrate.rs.
 * Threading:
 *   - Calls may come from any thread (the core is locked internally), except the
 *     "native_start" and "native_refresh_layout" commands, which must be sent on the
 *     main thread; the core checks this and returns an error otherwise.
 */
#ifndef TYPELUNE_CORE_H
#define TYPELUNE_CORE_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct TplCore TplCore;

uint32_t tpl_abi_version(void);
TplCore *tpl_core_open(const char *data_dir /* nullable */);
char *tpl_core_call(TplCore *core, const char *request_json);
void tpl_core_close(TplCore *core);
void tpl_string_free(char *s);

#ifdef __cplusplus
}
#endif

#endif /* TYPELUNE_CORE_H */
