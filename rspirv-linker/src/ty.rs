use crate::{extract_literal_int_as_u64, extract_literal_u32, DefAnalyzer};
use rspirv::spirv;

#[derive(PartialEq, Debug)]
pub enum ScalarType {
    Void,
    Bool,
    Int { width: u32, signed: bool },
    Float { width: u32 },
    Opaque { name: String },
    Event,
    DeviceEvent,
    ReserveId,
    Queue,
    Pipe,
    ForwardPointer { storage_class: spirv::StorageClass },
    PipeStorage,
    NamedBarrier,
    Sampler,
}

fn trans_scalar_type(inst: &rspirv::dr::Instruction) -> Option<ScalarType> {
    Some(match inst.class.opcode {
        spirv::Op::TypeVoid => ScalarType::Void,
        spirv::Op::TypeBool => ScalarType::Bool,
        spirv::Op::TypeEvent => ScalarType::Event,
        spirv::Op::TypeDeviceEvent => ScalarType::DeviceEvent,
        spirv::Op::TypeReserveId => ScalarType::ReserveId,
        spirv::Op::TypeQueue => ScalarType::Queue,
        spirv::Op::TypePipe => ScalarType::Pipe,
        spirv::Op::TypePipeStorage => ScalarType::PipeStorage,
        spirv::Op::TypeNamedBarrier => ScalarType::NamedBarrier,
        spirv::Op::TypeSampler => ScalarType::Sampler,
        spirv::Op::TypeForwardPointer => ScalarType::ForwardPointer {
            storage_class: match inst.operands[0] {
                rspirv::dr::Operand::StorageClass(s) => s,
                _ => panic!("Unexpected operand while parsing type"),
            },
        },
        spirv::Op::TypeInt => ScalarType::Int {
            width: match inst.operands[0] {
                rspirv::dr::Operand::LiteralInt32(w) => w,
                _ => panic!("Unexpected operand while parsing type"),
            },
            signed: match inst.operands[1] {
                rspirv::dr::Operand::LiteralInt32(s) => s != 0,
                _ => panic!("Unexpected operand while parsing type"),
            },
        },
        spirv::Op::TypeFloat => ScalarType::Float {
            width: match inst.operands[0] {
                rspirv::dr::Operand::LiteralInt32(w) => w,
                _ => panic!("Unexpected operand while parsing type"),
            },
        },
        spirv::Op::TypeOpaque => ScalarType::Opaque {
            name: match &inst.operands[0] {
                rspirv::dr::Operand::LiteralString(s) => s.clone(),
                _ => panic!("Unexpected operand while parsing type"),
            },
        },
        _ => return None,
    })
}

impl std::fmt::Display for ScalarType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            ScalarType::Void => f.write_str("void"),
            ScalarType::Bool => f.write_str("bool"),
            ScalarType::Int { width, signed } => {
                if signed {
                    write!(f, "i{}", width)
                } else {
                    write!(f, "u{}", width)
                }
            }
            ScalarType::Float { width } => write!(f, "f{}", width),
            ScalarType::Opaque { ref name } => write!(f, "Opaque{{{}}}", name),
            ScalarType::Event => f.write_str("Event"),
            ScalarType::DeviceEvent => f.write_str("DeviceEvent"),
            ScalarType::ReserveId => f.write_str("ReserveId"),
            ScalarType::Queue => f.write_str("Queue"),
            ScalarType::Pipe => f.write_str("Pipe"),
            ScalarType::ForwardPointer { storage_class } => {
                write!(f, "ForwardPointer{{{:?}}}", storage_class)
            }
            ScalarType::PipeStorage => f.write_str("PipeStorage"),
            ScalarType::NamedBarrier => f.write_str("NamedBarrier"),
            ScalarType::Sampler => f.write_str("Sampler"),
        }
    }
}

#[derive(PartialEq, Debug)]
#[allow(dead_code)]
pub enum AggregateType {
    Scalar(ScalarType),
    Array {
        ty: Box<AggregateType>,
        len: u64,
    },
    Pointer {
        ty: Box<AggregateType>,
        storage_class: spirv::StorageClass,
    },
    Image {
        ty: Box<AggregateType>,
        dim: spirv::Dim,
        depth: u32,
        arrayed: u32,
        multi_sampled: u32,
        sampled: u32,
        format: spirv::ImageFormat,
        access: Option<spirv::AccessQualifier>,
    },
    SampledImage {
        ty: Box<AggregateType>,
    },
    Aggregate(Vec<AggregateType>),
    Function(Vec<AggregateType>, Box<AggregateType>),
}

pub(crate) fn trans_aggregate_type(
    def: &DefAnalyzer,
    inst: &rspirv::dr::Instruction,
) -> Option<AggregateType> {
    Some(match inst.class.opcode {
        spirv::Op::TypeArray => {
            let len_def = def.op_def(&inst.operands[1]);
            assert!(len_def.class.opcode == spirv::Op::Constant); // don't support spec constants yet

            let len_value = extract_literal_int_as_u64(&len_def.operands[0]);

            AggregateType::Array {
                ty: Box::new(
                    trans_aggregate_type(def, &def.op_def(&inst.operands[0]))
                        .expect("Expect base type for OpTypeArray"),
                ),
                len: len_value,
            }
        }
        spirv::Op::TypePointer => AggregateType::Pointer {
            storage_class: match inst.operands[0] {
                rspirv::dr::Operand::StorageClass(s) => s,
                _ => panic!("Unexpected operand while parsing type"),
            },
            ty: Box::new(
                trans_aggregate_type(def, &def.op_def(&inst.operands[1]))
                    .expect("Expect base type for OpTypePointer"),
            ),
        },
        spirv::Op::TypeRuntimeArray
        | spirv::Op::TypeVector
        | spirv::Op::TypeMatrix
        | spirv::Op::TypeSampledImage => AggregateType::Aggregate(
            trans_aggregate_type(def, &def.op_def(&inst.operands[0]))
                .map_or_else(Vec::new, |v| vec![v]),
        ),
        spirv::Op::TypeStruct => {
            let mut types = vec![];
            for operand in inst.operands.iter() {
                let op_def = def.op_def(operand);

                match trans_aggregate_type(def, &op_def) {
                    Some(ty) => types.push(ty),
                    None => panic!("Expected type"),
                }
            }

            AggregateType::Aggregate(types)
        }
        spirv::Op::TypeFunction => {
            let mut parameters = vec![];
            let ret = trans_aggregate_type(def, &def.op_def(&inst.operands[0])).unwrap();
            for operand in inst.operands.iter().skip(1) {
                let op_def = def.op_def(operand);

                match trans_aggregate_type(def, &op_def) {
                    Some(ty) => parameters.push(ty),
                    None => panic!("Expected type"),
                }
            }

            AggregateType::Function(parameters, Box::new(ret))
        }
        spirv::Op::TypeImage => AggregateType::Image {
            ty: Box::new(
                trans_aggregate_type(def, &def.op_def(&inst.operands[0]))
                    .expect("Expect base type for OpTypeImage"),
            ),
            dim: match inst.operands[1] {
                rspirv::dr::Operand::Dim(d) => d,
                _ => panic!("Invalid dim"),
            },
            depth: extract_literal_u32(&inst.operands[2]),
            arrayed: extract_literal_u32(&inst.operands[3]),
            multi_sampled: extract_literal_u32(&inst.operands[4]),
            sampled: extract_literal_u32(&inst.operands[5]),
            format: match inst.operands[6] {
                rspirv::dr::Operand::ImageFormat(f) => f,
                _ => panic!("Invalid image format"),
            },
            access: inst
                .operands
                .get(7)
                .map(|op| match op {
                    rspirv::dr::Operand::AccessQualifier(a) => Some(*a),
                    _ => None,
                })
                .flatten(),
        },
        _ => {
            if let Some(ty) = trans_scalar_type(inst) {
                AggregateType::Scalar(ty)
            } else {
                return None;
            }
        }
    })
}

impl std::fmt::Display for AggregateType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AggregateType::Scalar(scalar) => write!(f, "{}", scalar),
            AggregateType::Array { ty, len } => write!(f, "[{}; {}]", ty, len),
            AggregateType::Pointer { ty, storage_class } => {
                write!(f, "*{{{:?}}} {}", storage_class, ty)
            }
            AggregateType::Image {
                ty,
                dim,
                depth,
                arrayed,
                multi_sampled,
                sampled,
                format,
                access,
            } => write!(
                f,
                "Image {{ {}, dim:{:?}, depth:{}, arrayed:{}, \
                multi_sampled:{}, sampled:{}, format:{:?}, access:{:?} }}",
                ty, dim, depth, arrayed, multi_sampled, sampled, format, access
            ),
            AggregateType::SampledImage { ty } => write!(f, "SampledImage{{{}}}", ty),
            AggregateType::Aggregate(agg) => {
                f.write_str("struct {")?;
                for elem in agg {
                    write!(f, " {},", elem)?;
                }
                f.write_str(" }")
            }
            AggregateType::Function(args, ret) => {
                f.write_str("fn(")?;
                for elem in args {
                    write!(f, " {},", elem)?;
                }
                write!(f, " ) -> {}", ret)
            }
        }
    }
}
