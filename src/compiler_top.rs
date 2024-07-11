use crate::prelude::*;

use tree_sitter::Parser;

use crate::{
    config::config, debug::SpanDebugger, errors::ErrorStore, file_position::FileText,
    linker::FileData,
};

use crate::flattening::{
    flatten_all_modules, gather_initial_file_data, typecheck_all_modules, Module,
};

pub fn add_file(text: String, linker: &mut Linker) -> FileUUID {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_sus::language()).unwrap();
    let tree = parser.parse(&text, None).unwrap();

    let file_id = linker.files.reserve();
    linker.files.alloc_reservation(
        file_id,
        FileData {
            parsing_errors: ErrorStore::new(),
            file_text: FileText::new(text),
            tree,
            associated_values: Vec::new(),
        },
    );

    linker.with_file_builder(file_id, |builder| {
        let mut span_debugger =
            SpanDebugger::new("gather_initial_file_data in add_file", builder.file_text);
        gather_initial_file_data(builder);
        span_debugger.defuse();
    });

    file_id
}

pub fn update_file(text: String, file_id: FileUUID, linker: &mut Linker) {
    let file_data = linker.remove_everything_in_file(file_id);

    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_sus::language()).unwrap();
    let tree = parser.parse(&text, None).unwrap();

    file_data.parsing_errors = ErrorStore::new();
    file_data.file_text = FileText::new(text);
    file_data.tree = tree;

    linker.with_file_builder(file_id, |builder| {
        let mut span_debugger =
            SpanDebugger::new("gather_initial_file_data in update_file", builder.file_text);
        gather_initial_file_data(builder);
        span_debugger.defuse();
    });
}

pub fn recompile_all(linker: &mut Linker) {
    // First reset all modules back to post-gather_initial_file_data
    for (_, md) in &mut linker.modules {
        let Module {
            link_info,
            instructions,
            instantiations,
            ..
        } = md;
        link_info.reset_to(link_info.after_initial_parse_cp);
        link_info.after_flatten_cp = None;
        instructions.clear();
        instantiations.clear_instances()
    }

    flatten_all_modules(linker);
    if config().debug_print_module_contents {
        for (_, md) in &linker.modules {
            md.print_flattened_module(&linker.files[md.link_info.file].file_text);
        }
    }

    typecheck_all_modules(linker);

    if config().debug_print_module_contents {
        for (_, md) in &linker.modules {
            md.print_flattened_module(&linker.files[md.link_info.file].file_text);
        }
    }

    // Make an initial instantiation of all modules
    // Won't be possible once we have template modules
    for (_id, md) in &linker.modules {
        //md.print_flattened_module();
        // Already instantiate any modules without parameters
        // Currently this is all modules
        let span_debug_message = format!("instantiating {}", &md.link_info.name);
        let mut span_debugger = SpanDebugger::new(
            &span_debug_message,
            &linker.files[md.link_info.file].file_text,
        );
        // Can immediately instantiate modules that have no template args
        if md.link_info.template_arguments.is_empty() {
            let _inst = md.instantiations.instantiate(md, linker, FlatAlloc::new());
        }
        span_debugger.defuse();
    }
}
