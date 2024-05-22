
mod name_context;
mod initialization;
mod typechecking;
mod parse;

pub use parse::flatten_all_modules;
pub use initialization::gather_initial_file_data;
pub use typechecking::typecheck_all_modules;

use crate::{
    arena_alloc::{FlatAlloc, UUIDMarker, UUIDRange, UUID}, errors::ErrorCollector, file_position::{BracketSpan, FileText, Span}, instantiation::InstantiationList, linker::{ConstantUUID, LinkInfo, ModuleUUID}, parser::Documentation, pretty_print_many_spans, typing::{AbstractType, WrittenType}, value::Value
};

/// Modules are compiled in 4 stages. All modules must pass through each stage before advancing to the next stage. 
/// 
/// 1. Initialization: initial name resolution and port discovery. The Module objects themselves are constructed. 
/// 
/// 2. Flattening: 
/// 
///     2.1: Parsing: Parse source code to create instruction list. 
/// 
///     2.2: Typecheck: Add typ variables to everything. [Declaration::typ], [WireInstance::typ] and [WireInstance::is_compiletime] are set in this stage. 
/// 
/// 3. Instantiation: Actually run generative code and instantiate modules. 
#[derive(Debug)]
pub struct Module {
    /// Created by Stage 1: Initialization
    pub link_info : LinkInfo,

    /// Created by Stage 1: Initialization
    pub ports : FlatAlloc<Port, PortIDMarker>,

    /// Created by Stage 1: Initialization
    pub interfaces : FlatAlloc<Interface, InterfaceIDMarker>,

    /// Created in Stage 2: Flattening and Typechecking
    pub instructions : FlatAlloc<Instruction, FlatIDMarker>,

    /// Created in Stage 3: Instantiation
    pub instantiations : InstantiationList
}

impl Module {
    pub const MAIN_INTERFACE_ID : InterfaceID = InterfaceID::from_hidden_value(0);
    
    pub fn get_port_decl(&self, port : PortID) -> &Declaration {
        let flat_port = self.ports[port].declaration_instruction;

        self.instructions[flat_port].unwrap_wire_declaration()
    }

    pub fn make_port_info_string(&self, port_id : PortID, file_text : &FileText) -> String {
        let port = &self.ports[port_id];
        let port_direction = if port.identifier_type == IdentifierType::Input {"input"} else {"output"};
        format!("{port_direction} {}", &file_text[port.decl_span])
    }

    pub fn make_interface_info_string(&self, interface_id : InterfaceID, file_text : &FileText) -> String {
        let mut result = String::new();

        for (port_id, port) in &self.ports {
            if port.interface == interface_id {
                result.push_str("\n    ");
                result.push_str(&self.make_port_info_string(port_id, file_text));
            }
        }

        result
    }

    pub fn make_all_ports_info_string(&self, file_text : &FileText) -> String {
        let mut result = self.make_interface_info_string(Module::MAIN_INTERFACE_ID, file_text);
        
        let mut interface_iter = self.interfaces.iter();
        // Already did main interface
        interface_iter.next();

        for (interface_id, interface) in interface_iter {
            result.push_str(&format!("\n    {}:", &interface.name));
            result.push_str(&self.make_interface_info_string(interface_id, file_text));
        }

        result
    }

    pub fn print_flattened_module(&self, file_text : &FileText) {
        println!("[[{}]]:", self.link_info.name);
        println!("Interface:");
        for (port_id, port) in &self.ports {
            println!("    {} -> {:?}", self.make_port_info_string(port_id, file_text), port);
        }
        println!("Instructions:");
        let mut spans_print = Vec::new();
        for (id, inst) in &self.instructions {
            println!("    {id:?}: {inst:?}");
            let span = self.get_instruction_span(id);
            spans_print.push((format!("{id:?}"), span.into_range()));
        }
        pretty_print_many_spans(file_text.file_text.clone(), &spans_print);
    }

    /// Get a port by the given name. Reports non existing ports errors
    pub fn get_port_by_name(&self, name_span : Span, file_text : &FileText, errors : &ErrorCollector) -> Option<PortID> {
        let name_text = &file_text[name_span];
        for (id, data) in &self.ports {
            if data.name == name_text {
                return Some(id)
            }
        }
        errors
            .error(name_span, format!("There is no port '{name_text}' on module {}", self.link_info.name))
            .info_obj(self);
        return None
    }

    pub fn get_instruction_span(&self, instr_id : FlatID) -> Span {
        match &self.instructions[instr_id] {
            Instruction::SubModule(sm) => sm.module_name_span,
            Instruction::Declaration(decl) => decl.get_span(),
            Instruction::Wire(w) => w.span,
            Instruction::Write(conn) => conn.to_span,
            Instruction::IfStatement(if_stmt) => self.get_instruction_span(if_stmt.condition),
            Instruction::ForStatement(for_stmt) => self.get_instruction_span(for_stmt.loop_var_decl),
        }
    }

    /// This function is intended to retrieve a known port while walking the syntax tree. panics if the port doesn't exist
    pub fn get_port_by_decl_span(&self, span : Span) -> PortID {
        for (id, data) in &self.ports {
            if data.decl_span == span {
                return id
            }
        }
        unreachable!()
    }
}


#[derive(Debug,Clone,Copy,PartialEq,Eq)]
pub enum IdentifierType {
    Input,
    Output,
    Local,
    State,
    Generative
}

impl IdentifierType {
    pub fn get_keyword(&self) -> &'static str {
        match self {
            IdentifierType::Input => "input",
            IdentifierType::Output => "output",
            IdentifierType::Local => "",
            IdentifierType::State => "state",
            IdentifierType::Generative => "gen",
        }
    }
    pub fn is_generative(&self) -> bool {
        *self == IdentifierType::Generative
    }
    pub fn is_port(&self) -> bool {
        *self == IdentifierType::Input || *self == IdentifierType::Output
    }
    #[track_caller]
    pub fn unwrap_is_input(&self) -> bool {
        match self {
            IdentifierType::Input => true,
            IdentifierType::Output => false,
            _ => unreachable!()
        }
    }
}


#[derive(Debug)]
pub struct Port {
    pub name : String,
    pub name_span : Span,
    pub decl_span : Span,
    pub identifier_type : IdentifierType,
    pub interface : InterfaceID,
    /// This is only set after flattening is done. Initially just [UUID::PLACEHOLDER]
    pub declaration_instruction : FlatID
}

#[derive(Debug)]
pub struct Interface {
    pub name_span : Span,
    pub name : String,
    pub func_call_inputs : PortIDRange,
    pub func_call_outputs : PortIDRange
}

pub struct FlatIDMarker;
impl UUIDMarker for FlatIDMarker {const DISPLAY_NAME : &'static str = "obj_";}
pub type FlatID = UUID<FlatIDMarker>;

pub type FlatIDRange = UUIDRange<FlatIDMarker>;

pub struct PortIDMarker;
impl UUIDMarker for PortIDMarker {const DISPLAY_NAME : &'static str = "port_";}
pub type PortID = UUID<PortIDMarker>;

pub type PortIDRange = UUIDRange<PortIDMarker>;

pub struct InterfaceIDMarker;
impl UUIDMarker for InterfaceIDMarker {const DISPLAY_NAME : &'static str = "port_";}
pub type InterfaceID = UUID<InterfaceIDMarker>;


#[derive(Debug, Clone, Copy)]
pub enum WireReferencePathElement {
    ArrayIdx{idx : FlatID, bracket_span : BracketSpan},
}

impl WireReferencePathElement {
    fn for_each_dependency<F : FnMut(FlatID)>(path : &[WireReferencePathElement], mut f : F) {
        for p in path {
            match p {
                WireReferencePathElement::ArrayIdx { idx, bracket_span:_ } => f(*idx),
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum WireReferenceRoot {
    LocalDecl(FlatID, Span),
    NamedConstant(ConstantUUID, Span),
    SubModulePort(PortInfo)
}

impl WireReferenceRoot {
    #[track_caller]
    pub fn unwrap_decl(&self) -> FlatID {
        let Self::LocalDecl(decl, _) = self else {unreachable!()};
        *decl
    }
    #[track_caller]
    pub fn unwrap_module_port(&self) -> &PortInfo {
        let Self::SubModulePort(port) = self else {unreachable!()};
        port
    }
}

impl WireReferenceRoot {
    pub fn get_root_flat(&self) -> Option<FlatID> {
        match self {
            WireReferenceRoot::LocalDecl(f, _) => Some(*f),
            WireReferenceRoot::NamedConstant(_, _) => None,
            WireReferenceRoot::SubModulePort(port) => Some(port.submodule_flat),
        }
    }
}

/// References to wires
/// 
/// Example: myModule.port[a][b:c]
#[derive(Debug)]
pub struct WireReference {
    pub root : WireReferenceRoot,
    pub path : Vec<WireReferencePathElement>
}

impl WireReference {
    fn simple_port(port : PortInfo) -> WireReference {
        WireReference{
            root : WireReferenceRoot::SubModulePort(port),
            path : Vec::new()
        }
    }
}

#[derive(Debug)]
pub enum WriteModifiers {
    Connection{num_regs : i64, regs_span : Span},
    Initial{initial_kw_span : Span}
}

#[derive(Debug)]
pub struct Write {
    pub from : FlatID,
    pub to : WireReference,
    pub to_span : Span,
    pub write_modifiers : WriteModifiers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOperator {
    And,
    Or,
    Xor,
    Not,
    Sum,
    Product,
    Negate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOperator {
    And,
    Or,
    Xor,
    Add,
    //ShiftLeft,
    //ShiftRight,
    Subtract,
    Multiply,
    Divide,
    Modulo,
    Equals,
    NotEquals,
    Greater,
    GreaterEq,
    Lesser,
    LesserEq
}

#[derive(Debug, Clone, Copy)]
pub struct PortInfo {
    /// Even this can be implicit. In the inline function call instantiation syntax there's no named submodule. my_mod(a, b, c)
    pub submodule_name_span : Option<Span>,
    pub submodule_flat : FlatID,
    pub port : PortID,
    /// Only set if the port is named as an explicit field. If the port name is implicit, such as in the function call syntax, then it is not present. 
    pub port_name_span : Option<Span>,
    pub port_identifier_typ : IdentifierType
}

#[derive(Debug)]
pub enum WireSource {
    WireRef(WireReference), // Used to add a span to the reference of a wire. 
    UnaryOp{op : UnaryOperator, right : FlatID},
    BinaryOp{op : BinaryOperator, left : FlatID, right : FlatID},
    Constant(Value)
}

impl WireSource {
    /// Enumerates all instructions that this instruction depends on. This includes (maybe compiletime) wires, and submodules. 
    pub fn for_each_dependency<F : FnMut(FlatID)>(&self, mut func : F) {
        match self {
            WireSource::WireRef(wire_ref) => {
                match &wire_ref.root {
                    WireReferenceRoot::LocalDecl(decl_id, _) => func(*decl_id),
                    WireReferenceRoot::NamedConstant(_, _) => {}
                    WireReferenceRoot::SubModulePort(submod_port) => func(submod_port.submodule_flat),
                }
                for p in &wire_ref.path {
                    match p {
                        WireReferencePathElement::ArrayIdx { idx, bracket_span:_ } => func(*idx),
                    }
                }
            }
            &WireSource::UnaryOp { op:_, right } => {func(right)}
            &WireSource::BinaryOp { op:_, left, right } => {func(left); func(right)},
            WireSource::Constant(_) => {}
        }
    }
}

const IS_GEN_UNINIT : bool = false;

#[derive(Debug)]
pub struct WireInstance {
    pub interface : InterfaceID,
    pub typ : AbstractType,
    pub is_compiletime : bool,
    pub span : Span,
    pub source : WireSource
}

#[derive(Debug)]
pub struct Declaration {
    pub interface : InterfaceID,
    pub typ_expr : WrittenType,
    pub typ : AbstractType,
    pub name_span : Span,
    pub name : String,
    /// Variables are read_only when they may not be controlled by the current block of code. 
    /// This is for example, the inputs of the current module, or the outputs of nested modules. 
    /// But could also be the iterator of a for loop. 
    pub read_only : bool,
    /// If the program text already covers the write, then lsp stuff on this declaration shouldn't use it. 
    pub declaration_itself_is_not_written_to : bool,
    pub identifier_type : IdentifierType,
    pub latency_specifier : Option<FlatID>,
    pub documentation : Documentation
}

impl Declaration {
    pub fn get_span(&self) -> Span {
        Span::new_overarching(self.typ_expr.get_span(), self.name_span).debug()
    }
}

#[derive(Debug)]
pub struct SubModuleInstance {
    pub module_uuid : ModuleUUID,
    /// Name is not always present in source code. Such as in inline function call syntax: my_mod(a, b, c)
    pub name : Option<(String, Span)>,
    pub module_name_span : Span,
    pub documentation : Documentation
}

#[derive(Debug)]
pub struct IfStatement {
    pub condition : FlatID,
    pub then_start : FlatID,
    pub then_end_else_start : FlatID,
    pub else_end : FlatID
}

#[derive(Debug)]
// Always is_compiletime
pub struct ForStatement {
    pub loop_var_decl : FlatID,
    pub start : FlatID,
    pub end : FlatID,
    pub loop_body : FlatIDRange
}

#[derive(Debug)]
pub enum Instruction {
    SubModule(SubModuleInstance),
    Declaration(Declaration),
    Wire(WireInstance),
    Write(Write),
    IfStatement(IfStatement),
    ForStatement(ForStatement)
}

impl Instruction {
    #[track_caller]
    pub fn unwrap_wire(&self) -> &WireInstance {
        let Self::Wire(w) = self else {panic!("unwrap_wire on not a wire! Found {self:?}")};
        w
    }
    #[track_caller]
    pub fn unwrap_wire_declaration(&self) -> &Declaration {
        let Self::Declaration(w) = self else {panic!("unwrap_wire_declaration on not a WireDeclaration! Found {self:?}")};
        w
    }
    #[track_caller]
    pub fn unwrap_submodule(&self) -> &SubModuleInstance {
        let Self::SubModule(sm) = self else {panic!("unwrap_submodule on not a SubModule! Found {self:?}")};
        sm
    }
    #[track_caller]
    pub fn unwrap_write(&self) -> &Write {
        let Self::Write(sm) = self else {panic!("unwrap_write on not a Write! Found {self:?}")};
        sm
    }

    pub fn for_each_embedded_type<F : FnMut(&AbstractType, Span)>(&self, mut f : F) {
        match self {
            Instruction::SubModule(_) | Instruction::Write(_) | Instruction::IfStatement(_) | Instruction::ForStatement(_) => {}
            Instruction::Declaration(decl) => {
                f(&decl.typ, decl.typ_expr.get_span());
            }
            Instruction::Wire(w) => {
                f(&w.typ, w.span);
            }
        }
    }
}


#[derive(Debug, Clone)]
pub enum ModuleOrWrittenType {
    WrittenType(WrittenType),
    Module(Span, ModuleUUID)
}
