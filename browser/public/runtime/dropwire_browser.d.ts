/* tslint:disable */
/* eslint-disable */
/**
 * The `ReadableStreamType` enum.
 *
 * *This API requires the following crate features to be activated: `ReadableStreamType`*
 */

export type ReadableStreamType = "bytes";

export class BrowserNode {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    add_file(name: string, size: number, read: Function, out_read: Function, out_write: Function): Promise<void>;
    cancel_handle(): CancelHandle;
    close(): Promise<void>;
    decline(): Promise<void>;
    inspect(input: string): Promise<string>;
    qr_svg(link: string): string;
    receive(selected: string, write: Function, progress: Function): Promise<void>;
    share(): Promise<string>;
    static spawn(relays: string): Promise<BrowserNode>;
    status(): string;
}

export class CancelHandle {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    cancel(): Promise<void>;
}

export class IntoUnderlyingByteSource {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    cancel(): void;
    pull(controller: ReadableByteStreamController): Promise<any>;
    start(controller: ReadableByteStreamController): void;
    readonly autoAllocateChunkSize: number;
    readonly type: ReadableStreamType;
}

export class IntoUnderlyingSink {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    abort(reason: any): Promise<any>;
    close(): Promise<any>;
    write(chunk: any): Promise<any>;
}

export class IntoUnderlyingSource {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    cancel(): void;
    pull(controller: ReadableStreamDefaultController): Promise<any>;
}

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_browsernode_free: (a: number, b: number) => void;
    readonly __wbg_cancelhandle_free: (a: number, b: number) => void;
    readonly browsernode_add_file: (a: number, b: number, c: number, d: number, e: any, f: any, g: any) => any;
    readonly browsernode_cancel_handle: (a: number) => number;
    readonly browsernode_close: (a: number) => any;
    readonly browsernode_decline: (a: number) => any;
    readonly browsernode_inspect: (a: number, b: number, c: number) => any;
    readonly browsernode_qr_svg: (a: number, b: number, c: number) => [number, number, number, number];
    readonly browsernode_receive: (a: number, b: number, c: number, d: any, e: any) => any;
    readonly browsernode_share: (a: number) => any;
    readonly browsernode_spawn: (a: number, b: number) => any;
    readonly browsernode_status: (a: number) => [number, number];
    readonly cancelhandle_cancel: (a: number) => any;
    readonly __wbg_intounderlyingsource_free: (a: number, b: number) => void;
    readonly intounderlyingsource_cancel: (a: number) => void;
    readonly intounderlyingsource_pull: (a: number, b: any) => any;
    readonly __wbg_intounderlyingsink_free: (a: number, b: number) => void;
    readonly intounderlyingsink_abort: (a: number, b: any) => any;
    readonly intounderlyingsink_close: (a: number) => any;
    readonly intounderlyingsink_write: (a: number, b: any) => any;
    readonly __wbg_intounderlyingbytesource_free: (a: number, b: number) => void;
    readonly intounderlyingbytesource_autoAllocateChunkSize: (a: number) => number;
    readonly intounderlyingbytesource_cancel: (a: number) => void;
    readonly intounderlyingbytesource_pull: (a: number, b: any) => any;
    readonly intounderlyingbytesource_start: (a: number, b: any) => void;
    readonly intounderlyingbytesource_type: (a: number) => number;
    readonly ring_core_0_17_14__bn_mul_mont: (a: number, b: number, c: number, d: number, e: number, f: number) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h426a2ac9f014f57a: (a: number, b: number, c: any) => [number, number];
    readonly wasm_bindgen__convert__closures_____invoke__h375a1c12e1a64648: (a: number, b: number, c: any, d: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__hd2dbeb3ab97b5c58: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h40aef191e4148fbd: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__hff3f3f26b5a708dd: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h2727bd29ffa19b04: (a: number, b: number) => void;
    readonly wasm_bindgen__convert__closures_____invoke__hd800e0d20381508d: (a: number, b: number) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h598255fb780a24c6: (a: number, b: number) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h283edd717a05bb96: (a: number, b: number) => void;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_exn_store: (a: number) => void;
    readonly __externref_table_alloc: () => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __wbindgen_destroy_closure: (a: number, b: number) => void;
    readonly __externref_table_dealloc: (a: number) => void;
    readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
