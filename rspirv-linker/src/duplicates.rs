use crate::operand_idref_mut;
use rspirv::spirv;
use std::collections::{hash_map, HashMap, HashSet};

pub fn remove_duplicate_capablities(module: &mut rspirv::dr::Module) {
    let mut set = HashSet::new();
    let mut caps = vec![];

    for c in &module.capabilities {
        let keep = match c.operands[0] {
            rspirv::dr::Operand::Capability(cap) => set.insert(cap),
            _ => true,
        };

        if keep {
            caps.push(c.clone());
        }
    }

    module.capabilities = caps;
}

pub fn remove_duplicate_ext_inst_imports(module: &mut rspirv::dr::Module) {
    // This is a simpler version of remove_duplicate_types, see that for comments
    let mut ext_to_id = HashMap::new();
    let mut rewrite_rules = HashMap::new();

    // First deduplicate the imports
    for inst in &mut module.ext_inst_imports {
        if let rspirv::dr::Operand::LiteralString(ext_inst_import) = &inst.operands[0] {
            match ext_to_id.entry(ext_inst_import.clone()) {
                hash_map::Entry::Vacant(entry) => {
                    entry.insert(inst.result_id.unwrap());
                }
                hash_map::Entry::Occupied(entry) => {
                    let old_value = rewrite_rules.insert(inst.result_id.unwrap(), *entry.get());
                    assert!(old_value.is_none());
                    *inst = rspirv::dr::Instruction::new(spirv::Op::Nop, None, None, vec![]);
                }
            }
        }
    }

    module
        .ext_inst_imports
        .retain(|op| op.class.opcode != spirv::Op::Nop);

    // Then rewrite all OpExtInst referencing the rewritten IDs
    for inst in module.all_inst_iter_mut() {
        if inst.class.opcode == spirv::Op::ExtInst {
            if let rspirv::dr::Operand::IdRef(ref mut id) = inst.operands[0] {
                *id = rewrite_rules.get(id).copied().unwrap_or(*id);
            }
        }
    }
}

// TODO: Don't merge zombie types with non-zombie types
pub fn remove_duplicate_types(module: &mut rspirv::dr::Module) {
    fn rewrite_inst_with_rules(inst: &mut rspirv::dr::Instruction, rules: &HashMap<u32, u32>) {
        if let Some(ref mut id) = inst.result_type {
            // If the rewrite rules contain this ID, replace with the mapped value, otherwise don't touch it.
            *id = rules.get(id).copied().unwrap_or(*id);
        }
        for op in &mut inst.operands {
            if let Some(id) = operand_idref_mut(op) {
                *id = rules.get(id).copied().unwrap_or(*id);
            }
        }
    }

    // Keep in mind, this algorithm requires forward type references to not exist - i.e. it's a valid spir-v module.
    use rspirv::binary::Assemble;

    // When a duplicate type is encountered, then this is a map from the deleted ID, to the new, deduplicated ID.
    let mut rewrite_rules = HashMap::new();
    // Instructions are encoded into "keys": their opcode, followed by arguments. Importantly, result_id is left out.
    // This means that any instruction that declares the same type, but with different result_id, will result in the
    // same key.
    let mut key_to_result_id = HashMap::new();
    // TODO: This is implementing forward pointers incorrectly.
    let mut unresolved_forward_pointers = HashSet::new();

    for inst in &mut module.types_global_values {
        if inst.class.opcode == spirv::Op::TypeForwardPointer {
            if let rspirv::dr::Operand::IdRef(id) = inst.operands[0] {
                unresolved_forward_pointers.insert(id);
                continue;
            }
        }
        if inst.class.opcode == spirv::Op::TypePointer
            && unresolved_forward_pointers.contains(&inst.result_id.unwrap())
        {
            unresolved_forward_pointers.remove(&inst.result_id.unwrap());
        }
        // This is an important spot: Say that we come upon a duplicated aggregate type (one that references
        // other types). Its arguments may be duplicated themselves, and so building the key directly will fail
        // to match up with the first type. However, **because forward references are not allowed**, we're
        // guaranteed to have already found and deduplicated the argument types! So that means the deduplication
        // translation is already in rewrite_rules, and we merely need to apply the mapping before generating
        // the key.
        // Nit: Overwriting the instruction isn't technically necessary, as it will get handled by the final
        // all_inst_iter_mut pass below. However, the code is a lil bit cleaner this way I guess.
        rewrite_inst_with_rules(inst, &rewrite_rules);

        let key = {
            let mut data = vec![];

            data.push(inst.class.opcode as u32);
            if let Some(id) = inst.result_type {
                // We're not only deduplicating types here, but constants as well. Those contain result_types, and so we
                // need to include those here. For example, OpConstant can have the same arg, but different result_type,
                // and it should not be deduplicated (e.g. the constants 1u8 and 1u16).
                data.push(id);
            }
            for op in &inst.operands {
                if let rspirv::dr::Operand::IdRef(id) = op {
                    if unresolved_forward_pointers.contains(id) {
                        // TODO: This is implementing forward pointers incorrectly. All unresolved forward pointers will
                        // compare equal.
                        rspirv::dr::Operand::IdRef(0).assemble_into(&mut data);
                    } else {
                        op.assemble_into(&mut data);
                    }
                } else {
                    op.assemble_into(&mut data);
                }
            }

            data
        };

        match key_to_result_id.entry(key) {
            hash_map::Entry::Vacant(entry) => {
                // This is the first time we've seen this key. Insert the key into the map, registering this type as
                // something other types can deduplicate to.
                entry.insert(inst.result_id.unwrap());
            }
            hash_map::Entry::Occupied(entry) => {
                // We've already seen this key. We need to do two things:
                // 1) Add a rewrite rule from this type to the type that we saw before.
                let old_value = rewrite_rules.insert(inst.result_id.unwrap(), *entry.get());
                // 2) Erase this instruction. Because we're iterating over this vec, removing an element is hard, so
                // clear it with OpNop, and then remove it in the retain() call below.
                assert!(old_value.is_none());
                *inst = rspirv::dr::Instruction::new(spirv::Op::Nop, None, None, vec![]);
            }
        }
    }

    // We rewrote instructions we wanted to remove with OpNop. Remove them properly.
    module
        .types_global_values
        .retain(|op| op.class.opcode != spirv::Op::Nop);

    // Apply the rewrite rules to the whole module
    for inst in module.all_inst_iter_mut() {
        rewrite_inst_with_rules(inst, &rewrite_rules);
    }
}
