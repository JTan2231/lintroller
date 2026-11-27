mod ffi;
mod imports;

pub use imports::ImportUsage;

use ffi::*;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::{CString, OsStr};
use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
use std::path::{Path, PathBuf};

#[derive(Debug, Hash, Eq, PartialEq)]
pub struct Declaration {
    pub path: PathBuf,
    pub line: u32,
    pub column: u32,
    pub name: String,
}

#[derive(Default)]
struct Symbols {
    used: HashSet<Vec<u8>>,
    // Empty extensions can appear as ordinary, relationless type references.
    bare_types: HashMap<PathBuf, Vec<TypeReference>>,
    // The same file can belong to several targets, giving one declaration
    // several USRs. A use of any of them keeps the declaration.
    declarations: HashMap<Declaration, Vec<Vec<u8>>>,
}

pub struct TypeReference {
    pub usr: Vec<u8>,
    pub line: u32,
    pub column: u32,
}

#[derive(Default)]
pub struct IndexedFile {
    pub candidates: Vec<(Declaration, Vec<Vec<u8>>)>,
    pub bare_types: Vec<TypeReference>,
    pub imports: ImportUsage,
}

impl Symbols {
    fn distribute(mut self, files: &mut BTreeMap<PathBuf, IndexedFile>) {
        self.declarations
            .retain(|_, usrs| usrs.iter().all(|u| !self.used.contains(u)));
        let pending: HashSet<&[u8]> = self
            .declarations
            .values()
            .flatten()
            .map(Vec::as_slice)
            .collect();
        for (path, mut references) in self.bare_types {
            references.retain(|r| pending.contains(r.usr.as_slice()));
            files.get_mut(&path).unwrap().bare_types = references;
        }
        for (decl, usrs) in self.declarations {
            files
                .get_mut(&decl.path)
                .unwrap()
                .candidates
                .push((decl, usrs));
        }
    }

    fn retain(&mut self, usr: &[u8]) {
        if !usr.is_empty() && !self.used.contains(usr) {
            self.used.insert(usr.to_vec());
        }
    }

    fn occurrence(
        &mut self,
        occ: Handle,
        paths: &[PathBuf],
        has_external_file: bool,
        imports: &mut [ImportUsage],
        check_declarations: bool,
    ) -> Result<(), String> {
        // All occurrence and symbol handles, including borrowed strings, are
        // valid until the record reader's synchronous callback returns.
        unsafe {
            let roles = indexstore_occurrence_get_roles(occ);
            let symbol = indexstore_occurrence_get_symbol(occ);
            let usr = indexstore_symbol_get_usr(symbol);
            let usr = usr.bytes();
            let kind = indexstore_symbol_get_kind(symbol);
            if roles & REFERENCE != 0 && !imports.is_empty() {
                let name = indexstore_symbol_get_name(symbol);
                let (mut line, mut column) = (0, 0);
                indexstore_occurrence_get_line_col(occ, &mut line, &mut column);
                for usage in imports {
                    usage.record(usr, kind, name.bytes(), line, column);
                }
            }
            if !check_declarations || usr.is_empty() {
                return Ok(());
            }
            if roles == REFERENCE
                && matches!(kind, kind::ENUM..=kind::PROTOCOL | kind::TYPE_ALIAS)
                && !has_external_file
            {
                let (mut line, mut column) = (0, 0);
                indexstore_occurrence_get_line_col(occ, &mut line, &mut column);
                for path in paths {
                    self.bare_types
                        .entry(path.clone())
                        .or_default()
                        .push(TypeReference {
                            usr: usr.to_vec(),
                            line,
                            column,
                        });
                }
            } else if roles & REFERENCE != 0 && roles & EXTENDED_BY == 0 || roles & OVERRIDE_OF != 0
            {
                // The compiler also emits overrideOf for protocol witnesses,
                // default implementations, and retroactive conformances.
                self.retain(usr);
            }
            if roles & BASE_OF != 0
                && matches!(usr, b"s:s9CodingKeyP" | b"s:7SwiftUI15PreviewProviderP")
            {
                visit(
                    |ctx, f| indexstore_occurrence_relations_apply_f(occ, ctx, f),
                    |relation| {
                        if indexstore_symbol_relation_get_roles(relation) & BASE_OF != 0 {
                            let derived = indexstore_symbol_relation_get_symbol(relation);
                            if usr != b"s:s9CodingKeyP"
                                || indexstore_symbol_get_name(derived).bytes() == b"CodingKeys"
                            {
                                self.retain(indexstore_symbol_get_usr(derived).bytes());
                            }
                        }
                        Ok(())
                    },
                )?;
            }
            if paths.is_empty()
                || roles & (DECLARATION | DEFINITION) == 0
                || roles & (IMPLICIT | ACCESSOR_OF) != 0
                || indexstore_symbol_get_language(symbol) != SWIFT
            {
                return Ok(());
            }
            // Swift macros, nominal types, typealiases, functions, variables,
            // and methods/properties. Exclude extension declarations, enum
            // cases, constructors, destructors, subscripts, and parameters.
            if !matches!(kind, kind::MACRO..=kind::PROTOCOL | kind::TYPE_ALIAS..=kind::FIELD | kind::INSTANCE_METHOD..=kind::STATIC_PROPERTY)
            {
                return Ok(());
            }
            let subkind = indexstore_symbol_get_subkind(symbol);
            // Ordinary declarations, operators, and associated types.
            if !matches!(
                subkind,
                subkind::NONE | subkind::PREFIX_OPERATOR
                    ..=subkind::INFIX_OPERATOR | subkind::ASSOCIATED_TYPE
            ) {
                return Ok(());
            }
            let properties = indexstore_symbol_get_properties(symbol);
            let name = indexstore_symbol_get_name(symbol);
            let name = std::str::from_utf8(name.bytes()).map_err(|e| e.to_string())?;
            // Objective-C callers and selectors may use Swift members through
            // the runtime. Let source policy handle Interface Builder outlets.
            let objc_member = matches!(kind, kind::INSTANCE_METHOD..=kind::STATIC_PROPERTY)
                && usr.starts_with(b"c:")
                && properties & IB_ANNOTATED == 0;
            if objc_member
                || properties & (UNIT_TEST | LOCAL) != 0
                || matches!(properties & ACCESS_MASK, ACCESS_PUBLIC | ACCESS_SPI)
            {
                self.retain(usr);
            }
            // Linux XCTest discovery also uses the static allTests property.
            if kind == kind::STATIC_PROPERTY && name == "allTests" && roles & CHILD_OF != 0 {
                visit(
                    |ctx, f| indexstore_occurrence_relations_apply_f(occ, ctx, f),
                    |relation| {
                        let owner = indexstore_symbol_relation_get_symbol(relation);
                        if indexstore_symbol_relation_get_roles(relation) & CHILD_OF != 0
                            && indexstore_symbol_get_properties(owner) & UNIT_TEST != 0
                        {
                            self.retain(usr);
                        }
                        Ok(())
                    },
                )?;
            }
            let (mut line, mut column) = (0, 0);
            indexstore_occurrence_get_line_col(occ, &mut line, &mut column);
            for path in paths {
                self.declarations
                    .entry(Declaration {
                        path: path.clone(),
                        line,
                        column,
                        name: name.to_owned(),
                    })
                    .or_default()
                    .push(usr.to_vec());
            }
            Ok(())
        }
    }
}

type Timestamp = (i64, i64);
#[derive(Default)]
struct Record {
    paths: HashMap<PathBuf, Timestamp>,
    modules: HashSet<String>,
}

#[derive(Default)]
struct Units {
    records: HashMap<Vec<u8>, Record>,
    present: HashSet<Vec<u8>>,
    required: HashSet<Vec<u8>>,
}

impl Units {
    fn collect(&mut self, store: Handle, unit_name: StringRef) -> Result<(), String> {
        unsafe {
            let name = CString::new(unit_name.bytes()).map_err(|e| e.to_string())?;
            self.present.insert(name.as_bytes().to_vec());
            let unit = Owned::open(
                |err| indexstore_unit_reader_create(store, name.as_ptr(), err),
                indexstore_unit_reader_dispose,
            )
            .map_err(|e| format!("unit {}: {e}", name.to_string_lossy()))?;
            if indexstore_unit_reader_is_system_unit(unit.raw) {
                return Ok(());
            }
            let (mut sec, mut ns) = (0, 0);
            indexstore_unit_reader_get_modification_time(unit.raw, &mut sec, &mut ns);
            let module = indexstore_unit_reader_get_module_name(unit.raw);
            let module = std::str::from_utf8(module.bytes()).map_err(|e| e.to_string())?;
            let working_dir = indexstore_unit_reader_get_working_dir(unit.raw);
            let working_dir = Path::new(OsStr::from_bytes(working_dir.bytes()));
            visit(
                |ctx, f| indexstore_unit_reader_dependencies_apply_f(unit.raw, ctx, f),
                |dep| {
                    if indexstore_unit_dependency_is_system(dep) {
                        return Ok(());
                    }
                    let dep_name = indexstore_unit_dependency_get_name(dep);
                    match indexstore_unit_dependency_get_kind(dep) {
                        DEPENDENCY_UNIT if !dep_name.bytes().is_empty() => {
                            self.required.insert(dep_name.bytes().to_vec());
                        }
                        DEPENDENCY_RECORD => {
                            let path = indexstore_unit_dependency_get_filepath(dep);
                            let path = working_dir.join(OsStr::from_bytes(path.bytes()));
                            let record = self.records.entry(dep_name.bytes().to_vec()).or_default();
                            if !module.is_empty() && !record.modules.contains(module) {
                                record.modules.insert(module.to_owned());
                            }
                            record
                                .paths
                                .entry(path)
                                .and_modify(|mtime| *mtime = (*mtime).min((sec, ns)))
                                .or_insert((sec, ns));
                        }
                        _ => {}
                    }
                    Ok(())
                },
            )
            .map_err(|e| format!("unit {}: {e}", name.to_string_lossy()))
        }
    }
}

fn project_source(
    path: &Path,
    root: &Path,
    timestamp: Timestamp,
) -> Result<Option<PathBuf>, String> {
    if path.extension() != Some(OsStr::new("swift")) {
        return Ok(None);
    }
    // Compiler records can reference generated files which no longer exist.
    // Resolve their existing prefix so /tmp and other symlinks still scope
    // missing project files correctly, without requiring reference-only files.
    let real = path.canonicalize().unwrap_or_else(|_| {
        path.ancestors()
            .skip(1)
            .find_map(|ancestor| {
                ancestor
                    .canonicalize()
                    .ok()
                    .map(|real| real.join(path.strip_prefix(ancestor).unwrap()))
            })
            .unwrap_or_else(|| path.to_owned())
    });
    let Ok(relative) = real.strip_prefix(root) else {
        return Ok(None);
    };
    if relative.components().any(|part| {
        matches!(
            part.as_os_str().as_bytes(),
            b".build" | b".swiftpm" | b"DerivedData"
        )
    }) {
        return Ok(None);
    }
    let metadata = real
        .metadata()
        .map_err(|e| format!("indexed source {}: {e}. Rebuild the index", real.display()))?;
    if (metadata.mtime(), metadata.mtime_nsec()) > timestamp {
        return Err(format!(
            "{} has changed since it was indexed. Rebuild the index",
            real.display()
        ));
    }
    Ok(Some(real))
}

pub fn read(
    index_store: &Path,
    project_root: &Path,
    declarations: bool,
    imports: bool,
) -> Result<BTreeMap<PathBuf, IndexedFile>, String> {
    let path = CString::new(index_store.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    let store = Owned::open(
        |err| unsafe { indexstore_store_create(path.as_ptr(), err) },
        indexstore_store_dispose,
    )
    .map_err(|e| format!("index store {}: {e}", index_store.display()))?;

    let mut units = Units::default();
    visit(
        |ctx, f| unsafe { indexstore_store_units_apply_f(store.raw, 0, ctx, f) },
        |name| units.collect(store.raw, name),
    )?;
    if units.present.is_empty() {
        return Err(format!(
            "no compilation units in {}. Pass the compiler's index store directory",
            index_store.display()
        ));
    }
    if let Some(missing) = units.required.difference(&units.present).next() {
        return Err(format!(
            "missing index unit {}. Rebuild the index",
            String::from_utf8_lossy(missing)
        ));
    }

    let mut symbols = Symbols::default();
    let mut sources = BTreeMap::<PathBuf, IndexedFile>::new();
    for (name, record_info) in units.records {
        let mut paths = Vec::new();
        let mut has_external_file = false;
        for (path, timestamp) in record_info.paths {
            if let Some(path) = project_source(&path, project_root, timestamp)? {
                paths.push(path);
            } else {
                has_external_file = true;
            }
        }
        paths.sort_unstable();
        paths.dedup();
        let mut usage = Vec::new();
        for path in &paths {
            let file = sources.entry(path.clone()).or_default();
            if imports {
                for module in &record_info.modules {
                    file.imports.declared_in(module);
                }
                usage.push(std::mem::take(&mut file.imports));
            }
        }
        if !declarations && (!imports || paths.is_empty()) {
            continue;
        }
        let name = CString::new(name).map_err(|e| e.to_string())?;
        let record = Owned::open(
            |err| unsafe { indexstore_record_reader_create(store.raw, name.as_ptr(), err) },
            indexstore_record_reader_dispose,
        )
        .map_err(|e| format!("record {}: {e}", name.to_string_lossy()))?;

        let result = visit(
            |ctx, f| unsafe { indexstore_record_reader_occurrences_apply_f(record.raw, ctx, f) },
            |occ| symbols.occurrence(occ, &paths, has_external_file, &mut usage, declarations),
        );
        for (path, usage) in paths.iter().zip(usage) {
            sources.get_mut(path).unwrap().imports = usage;
        }
        if let Err(err) = result {
            // libIndexStore also returns false for a valid record with no
            // occurrence block (e.g. a Swift file containing only comments).
            let mut empty = true;
            visit(
                |ctx, f| unsafe {
                    indexstore_record_reader_symbols_apply_f(record.raw, true, ctx, f)
                },
                |_| {
                    empty = false;
                    Ok(())
                },
            )?;
            if !empty {
                return Err(format!("record {}: {err}", name.to_string_lossy()));
            }
        }
    }
    if sources.is_empty() {
        return Err(format!(
            "no indexed Swift sources under {}",
            project_root.display()
        ));
    }
    symbols.distribute(&mut sources);
    Ok(sources)
}
