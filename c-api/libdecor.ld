/* Version script controlling which symbols libdecor-0.so exports.
 *
 * Only the documented libdecor C ABI is visible to dynamic consumers.
 * Every other Rust symbol becomes local so the linker can
 * dead-code-eliminate code reachable only through it. */

{
    global:
        libdecor_*;
    local:
        *;
};
