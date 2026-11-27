//! The synchronous subset of clang/include/indexstore/indexstore.h that we use.
//! Xcode ships libIndexStore without its public header.
use std::ffi::{CStr, c_char, c_void};
use std::ptr;
use std::slice;

// Values from indexstore_symbol_kind_t and indexstore_symbol_subkind_t.
pub mod kind {
    pub const MACRO: u32 = 4;
    pub const ENUM: u32 = 5;
    pub const PROTOCOL: u32 = 8;
    pub const TYPE_ALIAS: u32 = 11;
    pub const FIELD: u32 = 14;
    pub const INSTANCE_METHOD: u32 = 16;
    pub const STATIC_PROPERTY: u32 = 21;
}

pub mod subkind {
    pub const NONE: u32 = 0;
    pub const PREFIX_OPERATOR: u32 = 1008;
    pub const INFIX_OPERATOR: u32 = 1010;
    pub const ASSOCIATED_TYPE: u32 = 1012;
}

pub const DEPENDENCY_UNIT: u32 = 1;
pub const DEPENDENCY_RECORD: u32 = 2;

pub const DECLARATION: u64 = 1 << 0;
pub const DEFINITION: u64 = 1 << 1;
pub const REFERENCE: u64 = 1 << 2;
pub const IMPLICIT: u64 = 1 << 8;
pub const CHILD_OF: u64 = 1 << 9;
pub const BASE_OF: u64 = 1 << 10;
pub const OVERRIDE_OF: u64 = 1 << 11;
pub const EXTENDED_BY: u64 = 1 << 14;
pub const ACCESSOR_OF: u64 = 1 << 15;

pub const SWIFT: u32 = 100;
pub const IB_ANNOTATED: u64 = (1 << 4) | (1 << 5) | (1 << 6);
pub const UNIT_TEST: u64 = 1 << 3;
pub const LOCAL: u64 = 1 << 7;
pub const ACCESS_MASK: u64 = 0b111 << 17;
pub const ACCESS_PUBLIC: u64 = 0b110 << 17;
pub const ACCESS_SPI: u64 = 0b101 << 17;

pub type Handle = *mut c_void;
pub type Visitor<T> = unsafe extern "C" fn(Handle, T) -> bool;

#[derive(Clone, Copy)]
#[repr(C)]
pub struct StringRef {
    data: *const u8,
    length: usize,
}

impl StringRef {
    // The owning libIndexStore handle must still be alive.
    pub unsafe fn bytes(&self) -> &[u8] {
        if self.length == 0 {
            &[]
        } else {
            unsafe { slice::from_raw_parts(self.data, self.length) }
        }
    }
}

unsafe extern "C" {
    fn indexstore_error_get_description(error: Handle) -> *const c_char;
    fn indexstore_error_dispose(error: Handle);

    pub fn indexstore_store_create(path: *const c_char, error: *mut Handle) -> Handle;
    pub fn indexstore_store_dispose(store: Handle);
    pub fn indexstore_store_units_apply_f(
        store: Handle,
        sorted: u32,
        ctx: Handle,
        f: Visitor<StringRef>,
    ) -> bool;
    pub fn indexstore_unit_reader_create(
        store: Handle,
        name: *const c_char,
        err: *mut Handle,
    ) -> Handle;
    pub fn indexstore_unit_reader_dispose(unit: Handle);
    pub fn indexstore_unit_reader_is_system_unit(unit: Handle) -> bool;
    pub fn indexstore_unit_reader_get_module_name(unit: Handle) -> StringRef;
    pub fn indexstore_unit_reader_get_working_dir(unit: Handle) -> StringRef;
    pub fn indexstore_unit_reader_get_modification_time(
        unit: Handle,
        sec: *mut i64,
        nsec: *mut i64,
    );
    pub fn indexstore_unit_reader_dependencies_apply_f(
        unit: Handle,
        ctx: Handle,
        f: Visitor<Handle>,
    ) -> bool;
    pub fn indexstore_unit_dependency_get_kind(dep: Handle) -> u32;
    pub fn indexstore_unit_dependency_is_system(dep: Handle) -> bool;
    pub fn indexstore_unit_dependency_get_name(dep: Handle) -> StringRef;
    pub fn indexstore_unit_dependency_get_filepath(dep: Handle) -> StringRef;

    pub fn indexstore_record_reader_create(
        store: Handle,
        name: *const c_char,
        err: *mut Handle,
    ) -> Handle;
    pub fn indexstore_record_reader_dispose(record: Handle);
    pub fn indexstore_record_reader_symbols_apply_f(
        record: Handle,
        nocache: bool,
        ctx: Handle,
        f: Visitor<Handle>,
    ) -> bool;
    pub fn indexstore_record_reader_occurrences_apply_f(
        record: Handle,
        ctx: Handle,
        f: Visitor<Handle>,
    ) -> bool;
    pub fn indexstore_occurrence_get_symbol(occ: Handle) -> Handle;
    pub fn indexstore_occurrence_get_roles(occ: Handle) -> u64;
    pub fn indexstore_occurrence_get_line_col(occ: Handle, line: *mut u32, col: *mut u32);
    pub fn indexstore_occurrence_relations_apply_f(
        occ: Handle,
        ctx: Handle,
        f: Visitor<Handle>,
    ) -> bool;
    pub fn indexstore_symbol_relation_get_roles(relation: Handle) -> u64;
    pub fn indexstore_symbol_relation_get_symbol(relation: Handle) -> Handle;
    pub fn indexstore_symbol_get_language(symbol: Handle) -> u32;
    pub fn indexstore_symbol_get_kind(symbol: Handle) -> u32;
    pub fn indexstore_symbol_get_subkind(symbol: Handle) -> u32;
    pub fn indexstore_symbol_get_properties(symbol: Handle) -> u64;
    pub fn indexstore_symbol_get_usr(symbol: Handle) -> StringRef;
    pub fn indexstore_symbol_get_name(symbol: Handle) -> StringRef;
}

pub struct Owned {
    pub raw: Handle,
    dispose: unsafe extern "C" fn(Handle),
}

impl Owned {
    pub fn open(
        create: impl FnOnce(*mut Handle) -> Handle,
        dispose: unsafe extern "C" fn(Handle),
    ) -> Result<Self, String> {
        let mut err = ptr::null_mut();
        let raw = create(&mut err);
        if raw.is_null() {
            if err.is_null() {
                return Err("libIndexStore could not open reader".into());
            }
            unsafe {
                let message = CStr::from_ptr(indexstore_error_get_description(err))
                    .to_string_lossy()
                    .into_owned();
                indexstore_error_dispose(err);
                Err(message)
            }
        } else {
            Ok(Self { raw, dispose })
        }
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        unsafe { (self.dispose)(self.raw) }
    }
}

// Each *_apply_f API invokes the visitor synchronously and returns false if
// traversal stopped early. The C library never retains the callback context.
pub fn visit<T, F: FnMut(T) -> Result<(), String>>(
    apply: impl FnOnce(Handle, Visitor<T>) -> bool,
    visitor: F,
) -> Result<(), String> {
    unsafe extern "C" fn call<T, F: FnMut(T) -> Result<(), String>>(ctx: Handle, value: T) -> bool {
        let (visit, result) = unsafe { &mut *ctx.cast::<(F, Result<(), String>)>() };
        *result = visit(value);
        result.is_ok()
    }
    let mut state: (F, Result<(), String>) = (visitor, Ok(()));
    let complete = apply((&raw mut state).cast(), call::<T, F>);
    state.1?;
    complete
        .then_some(())
        .ok_or_else(|| "incomplete index traversal".into())
}
