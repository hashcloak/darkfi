use std::{collections::HashMap, convert::TryInto};

use halo2::{
    circuit::{Layouter, SimpleFloorPlanner},
    plonk,
    plonk::{Advice, Circuit, Column, ConstraintSystem, Instance as InstanceColumn, Selector},
};
use halo2_gadgets::{
    ecc::{
        chip::{EccChip, EccConfig},
        FixedPoint,
    },
    poseidon::{Hash as PoseidonHash, Pow5T3Chip as PoseidonChip, Pow5T3Config as PoseidonConfig},
    primitives::poseidon::{ConstantLength, P128Pow5T3},
    sinsemilla::{
        chip::{SinsemillaChip, SinsemillaConfig},
        merkle::{
            chip::{MerkleChip, MerkleConfig},
            MerklePath,
        },
    },
    utilities::{
        lookup_range_check::LookupRangeCheckConfig, CellValue, UtilitiesInstructions, Var,
    },
};
use pasta_curves::pallas;

use crate::{
    crypto::{
        arith_chip::{ArithmeticChip, ArithmeticChipConfig},
        constants::{
            sinsemilla::{OrchardCommitDomains, OrchardHashDomains},
            OrchardFixedBases,
        },
    },
    error::{Error, Result},
};

#[derive(Clone, Debug, PartialEq)]
pub enum ZkType {
    Base,
    Scalar,
    EcPoint,
    EcFixedPoint,
    MerklePath,
}

type ArgIdx = usize;

#[derive(Clone, Debug)]
pub enum ZkFunctionCall {
    PoseidonHash(ArgIdx, ArgIdx),
    Add(ArgIdx, ArgIdx),
    ConstrainInstance(ArgIdx),
    EcMulShort(ArgIdx, ArgIdx),
    EcMul(ArgIdx, ArgIdx),
    EcAdd(ArgIdx, ArgIdx),
    EcGetX(ArgIdx),
    EcGetY(ArgIdx),
    CalculateMerkleRoot(ArgIdx, ArgIdx),
}

pub struct ZkBinary {
    pub constants: Vec<(String, ZkType)>,
    pub contracts: HashMap<String, ZkContract>,
}

#[derive(Clone, Debug)]
pub struct ZkContract {
    pub witness: Vec<(String, ZkType)>,
    pub code: Vec<ZkFunctionCall>,
}

// These is the actual structures below which interpret the structures
// deserialized above.

#[derive(Clone, Debug)]
pub struct MintConfig {
    pub primary: Column<InstanceColumn>,
    pub q_add: Selector,
    pub advices: [Column<Advice>; 10],
    pub ecc_config: EccConfig,
    pub merkle_config_1: MerkleConfig<OrchardHashDomains, OrchardCommitDomains, OrchardFixedBases>,
    pub merkle_config_2: MerkleConfig<OrchardHashDomains, OrchardCommitDomains, OrchardFixedBases>,
    pub sinsemilla_config_1:
        SinsemillaConfig<OrchardHashDomains, OrchardCommitDomains, OrchardFixedBases>,
    pub sinsemilla_config_2:
        SinsemillaConfig<OrchardHashDomains, OrchardCommitDomains, OrchardFixedBases>,
    pub poseidon_config: PoseidonConfig<pallas::Base>,
    pub arith_config: ArithmeticChipConfig,
}

impl MintConfig {
    pub fn ecc_chip(&self) -> EccChip<OrchardFixedBases> {
        EccChip::construct(self.ecc_config.clone())
    }

    pub fn poseidon_chip(&self) -> PoseidonChip<pallas::Base> {
        PoseidonChip::construct(self.poseidon_config.clone())
    }

    pub fn arithmetic_chip(&self) -> ArithmeticChip {
        ArithmeticChip::construct(self.arith_config.clone())
    }

    fn merkle_chip_1(
        &self,
    ) -> MerkleChip<OrchardHashDomains, OrchardCommitDomains, OrchardFixedBases> {
        MerkleChip::construct(self.merkle_config_1.clone())
    }

    fn merkle_chip_2(
        &self,
    ) -> MerkleChip<OrchardHashDomains, OrchardCommitDomains, OrchardFixedBases> {
        MerkleChip::construct(self.merkle_config_2.clone())
    }
}

#[derive(Clone, Debug)]
pub struct ZkCircuit<'a> {
    pub const_fixed_points: HashMap<String, OrchardFixedBases>,
    pub constants: &'a [(String, ZkType)],
    pub contract: &'a ZkContract,
    // For each type create a separate stack
    pub witness_base: HashMap<String, Option<pallas::Base>>,
    pub witness_scalar: HashMap<String, Option<pallas::Scalar>>,
    pub witness_merkle_path: HashMap<String, (Option<u32>, Option<[pallas::Base; 32]>)>,
}

impl<'a> ZkCircuit<'a> {
    pub fn new(
        const_fixed_points: HashMap<String, OrchardFixedBases>,
        constants: &'a [(String, ZkType)],
        contract: &'a ZkContract,
    ) -> Self {
        let mut witness_base = HashMap::new();
        let mut witness_scalar = HashMap::new();
        let mut witness_merkle_path = HashMap::new();
        for (name, type_id) in contract.witness.iter() {
            match type_id {
                ZkType::Base => {
                    witness_base.insert(name.clone(), None);
                }
                ZkType::Scalar => {
                    witness_scalar.insert(name.clone(), None);
                }
                ZkType::MerklePath => {
                    witness_merkle_path.insert(name.clone(), (None, None));
                }
                _ => {
                    unimplemented!();
                }
            }
        }

        Self {
            const_fixed_points,
            constants,
            contract,
            witness_base,
            witness_scalar,
            witness_merkle_path,
        }
    }

    pub fn witness_base(&mut self, name: &str, value: pallas::Base) -> Result<()> {
        for (variable, type_id) in self.contract.witness.iter() {
            if name != variable {
                continue
            }
            if *type_id != ZkType::Base {
                return Err(Error::InvalidParamType)
            }
            *self.witness_base.get_mut(name).unwrap() = Some(value);
            return Ok(())
        }
        Err(Error::InvalidParamName)
    }

    pub fn witness_scalar(&mut self, name: &str, value: pallas::Scalar) -> Result<()> {
        for (variable, type_id) in self.contract.witness.iter() {
            if name != variable {
                continue
            }
            if *type_id != ZkType::Scalar {
                return Err(Error::InvalidParamType)
            }
            *self.witness_scalar.get_mut(name).unwrap() = Some(value);
            return Ok(())
        }
        Err(Error::InvalidParamName)
    }

    pub fn witness_merkle_path(
        &mut self,
        name: &str,
        leaf_pos: u32,
        path: [pallas::Base; 32],
    ) -> Result<()> {
        for (variable, type_id) in self.contract.witness.iter() {
            if name != variable {
                continue
            }
            if *type_id != ZkType::MerklePath {
                return Err(Error::InvalidParamType)
            }
            *self.witness_merkle_path.get_mut(name).unwrap() = (Some(leaf_pos), Some(path));
            return Ok(())
        }
        Err(Error::InvalidParamName)
    }
}

impl<'a> UtilitiesInstructions<pallas::Base> for ZkCircuit<'a> {
    type Var = CellValue<pallas::Base>;
}

impl<'a> Circuit<pallas::Base> for ZkCircuit<'a> {
    type Config = MintConfig;
    type FloorPlanner = SimpleFloorPlanner;

    fn without_witnesses(&self) -> Self {
        Self {
            const_fixed_points: self.const_fixed_points.clone(),
            constants: self.constants,
            contract: self.contract,
            witness_base: self.witness_base.keys().map(|key| (key.clone(), None)).collect(),
            witness_scalar: self.witness_scalar.keys().map(|key| (key.clone(), None)).collect(),
            witness_merkle_path: self
                .witness_scalar
                .keys()
                .map(|key| (key.clone(), (None, None)))
                .collect(),
        }
    }

    fn configure(meta: &mut ConstraintSystem<pallas::Base>) -> Self::Config {
        let advices = [
            meta.advice_column(),
            meta.advice_column(),
            meta.advice_column(),
            meta.advice_column(),
            meta.advice_column(),
            meta.advice_column(),
            meta.advice_column(),
            meta.advice_column(),
            meta.advice_column(),
            meta.advice_column(),
        ];

        let q_add = meta.selector();

        let table_idx = meta.lookup_table_column();
        let lookup = (table_idx, meta.lookup_table_column(), meta.lookup_table_column());

        let primary = meta.instance_column();

        meta.enable_equality(primary.into());

        for advice in advices.iter() {
            meta.enable_equality((*advice).into());
        }

        let lagrange_coeffs = [
            meta.fixed_column(),
            meta.fixed_column(),
            meta.fixed_column(),
            meta.fixed_column(),
            meta.fixed_column(),
            meta.fixed_column(),
            meta.fixed_column(),
            meta.fixed_column(),
        ];

        let rc_a = lagrange_coeffs[2..5].try_into().unwrap();
        let rc_b = lagrange_coeffs[5..8].try_into().unwrap();

        meta.enable_constant(lagrange_coeffs[0]);

        let range_check = LookupRangeCheckConfig::configure(meta, advices[9], table_idx);

        let ecc_config = EccChip::<OrchardFixedBases>::configure(
            meta,
            advices,
            lagrange_coeffs,
            range_check.clone(),
        );

        let poseidon_config = PoseidonChip::configure(
            meta,
            P128Pow5T3,
            advices[6..9].try_into().unwrap(),
            advices[5],
            rc_a,
            rc_b,
        );

        let arith_config = ArithmeticChip::configure(meta);

        // Configuration for a Sinsemilla hash instantiation and a
        // Merkle hash instantiation using this Sinsemilla instance.
        // Since the Sinsemilla config uses only 5 advice columns,
        // we can fit two instances side-by-side.
        let (sinsemilla_config_1, merkle_config_1) = {
            let sinsemilla_config_1 = SinsemillaChip::configure(
                meta,
                advices[..5].try_into().unwrap(),
                advices[6],
                lagrange_coeffs[0],
                lookup,
                range_check.clone(),
            );
            let merkle_config_1 = MerkleChip::configure(meta, sinsemilla_config_1.clone());
            (sinsemilla_config_1, merkle_config_1)
        };

        // Configuration for a Sinsemilla hash instantiation and a
        // Merkle hash instantiation using this Sinsemilla instance.
        // Since the Sinsemilla config uses only 5 advice columns,
        // we can fit two instances side-by-side.
        let (sinsemilla_config_2, merkle_config_2) = {
            let sinsemilla_config_2 = SinsemillaChip::configure(
                meta,
                advices[5..].try_into().unwrap(),
                advices[7],
                lagrange_coeffs[1],
                lookup,
                range_check,
            );
            let merkle_config_2 = MerkleChip::configure(meta, sinsemilla_config_2.clone());

            (sinsemilla_config_2, merkle_config_2)
        };

        MintConfig {
            primary,
            q_add,
            advices,
            ecc_config,
            merkle_config_1,
            merkle_config_2,
            sinsemilla_config_1,
            sinsemilla_config_2,
            poseidon_config,
            arith_config,
        }
    }

    fn synthesize(
        &self,
        config: Self::Config,
        mut layouter: impl Layouter<pallas::Base>,
    ) -> std::result::Result<(), plonk::Error> {
        // Load the Sinsemilla generator lookup table used by the whole circuit.
        SinsemillaChip::load(config.sinsemilla_config_1.clone(), &mut layouter)?;

        let arith_chip = config.arithmetic_chip();

        // Construct the ECC chip.
        let ecc_chip = config.ecc_chip();

        let mut stack_base = Vec::new();
        let mut stack_scalar = Vec::new();
        let mut stack_ec_point = Vec::new();
        let mut stack_ec_fixed_point = Vec::new();
        let mut stack_merkle_path = Vec::new();

        // Load constants first onto the stacks
        for (variable, type_id) in self.constants.iter() {
            match *type_id {
                ZkType::Base => {
                    unimplemented!();
                }
                ZkType::Scalar => {
                    unimplemented!();
                }
                ZkType::EcPoint => {
                    unimplemented!();
                }
                ZkType::EcFixedPoint => {
                    let value = self.const_fixed_points[variable];
                    stack_ec_fixed_point.push(value);
                }
                ZkType::MerklePath => {
                    unimplemented!();
                }
            }
        }

        // Push the witnesses onto the stacks in order
        for (variable, type_id) in self.contract.witness.iter() {
            match *type_id {
                ZkType::Base => {
                    let value = self.witness_base.get(variable).expect("witness base set");
                    let value = self.load_private(
                        layouter.namespace(|| "load pubkey x"),
                        config.advices[0],
                        *value,
                    )?;
                    stack_base.push(value);
                }
                ZkType::Scalar => {
                    let value = self.witness_scalar.get(variable).expect("witness base set");
                    stack_scalar.push(*value);
                }
                ZkType::EcPoint => {
                    unimplemented!();
                }
                ZkType::EcFixedPoint => {
                    unimplemented!();
                }
                ZkType::MerklePath => {
                    let value =
                        self.witness_merkle_path.get(variable).expect("witness merkle path set");
                    stack_merkle_path.push(*value);
                }
            }
        }

        let mut current_instance_offset = 0;

        for func_call in self.contract.code.iter() {
            match func_call {
                ZkFunctionCall::PoseidonHash(lhs_idx, rhs_idx) => {
                    assert!(*lhs_idx < stack_base.len());
                    assert!(*rhs_idx < stack_base.len());
                    let poseidon_message = [stack_base[*lhs_idx], stack_base[*rhs_idx]];

                    let poseidon_hasher = PoseidonHash::<_, _, P128Pow5T3, _, 3, 2>::init(
                        config.poseidon_chip(),
                        layouter.namespace(|| "Poseidon init"),
                        ConstantLength::<2>,
                    )?;

                    let poseidon_output = poseidon_hasher
                        .hash(layouter.namespace(|| "poseidon hash"), poseidon_message)?;

                    let poseidon_output: CellValue<pallas::Base> = poseidon_output.inner().into();
                    stack_base.push(poseidon_output);
                }
                ZkFunctionCall::Add(lhs_idx, rhs_idx) => {
                    assert!(*lhs_idx < stack_base.len());
                    assert!(*rhs_idx < stack_base.len());
                    let (lhs, rhs) = (stack_base[*lhs_idx], stack_base[*rhs_idx]);
                    let output =
                        arith_chip.add(layouter.namespace(|| "arithmetic add"), lhs, rhs)?;
                    stack_base.push(output);
                }
                ZkFunctionCall::ConstrainInstance(arg_idx) => {
                    assert!(*arg_idx < stack_base.len());
                    let arg = stack_base[*arg_idx];
                    layouter.constrain_instance(
                        arg.cell(),
                        config.primary,
                        current_instance_offset,
                    )?;
                    current_instance_offset += 1;
                }
                ZkFunctionCall::EcMulShort(value_idx, point_idx) => {
                    assert!(*value_idx < stack_base.len());
                    let value = stack_base[*value_idx];

                    assert!(*point_idx < stack_ec_fixed_point.len());
                    let fixed_point = stack_ec_fixed_point[*point_idx];

                    // This constant one is used for multiplication
                    let one = self.load_private(
                        layouter.namespace(|| "load constant one"),
                        config.advices[0],
                        Some(pallas::Base::one()),
                    )?;

                    // v * G_1
                    let (result, _) = {
                        let value_commit_v = FixedPoint::from_inner(ecc_chip.clone(), fixed_point);
                        value_commit_v.mul_short(
                            layouter.namespace(|| "[value] ValueCommitV"),
                            (value, one),
                        )?
                    };

                    stack_ec_point.push(result);
                }
                ZkFunctionCall::EcMul(value_idx, point_idx) => {
                    assert!(*value_idx < stack_scalar.len());
                    let value = stack_scalar[*value_idx];

                    assert!(*point_idx < stack_ec_fixed_point.len());
                    let fixed_point = stack_ec_fixed_point[*point_idx];

                    let (result, _) = {
                        let value_commit_r = FixedPoint::from_inner(ecc_chip.clone(), fixed_point);
                        value_commit_r
                            .mul(layouter.namespace(|| "[value_blind] ValueCommitR"), value)?
                    };

                    stack_ec_point.push(result);
                }
                ZkFunctionCall::EcAdd(lhs_idx, rhs_idx) => {
                    assert!(*lhs_idx < stack_ec_point.len());
                    assert!(*rhs_idx < stack_ec_point.len());
                    let lhs = &stack_ec_point[*lhs_idx];
                    let rhs = &stack_ec_point[*rhs_idx];

                    let result = lhs.add(layouter.namespace(|| "valuecommit"), rhs)?;
                    stack_ec_point.push(result);
                }
                ZkFunctionCall::EcGetX(arg_idx) => {
                    assert!(*arg_idx < stack_ec_point.len());
                    let arg = &stack_ec_point[*arg_idx];
                    let x = arg.inner().x();
                    stack_base.push(x);
                }
                ZkFunctionCall::EcGetY(arg_idx) => {
                    assert!(*arg_idx < stack_ec_point.len());
                    let arg = &stack_ec_point[*arg_idx];
                    let y = arg.inner().y();
                    stack_base.push(y);
                }
                ZkFunctionCall::CalculateMerkleRoot(path_idx, leaf_idx) => {
                    assert!(*path_idx < stack_merkle_path.len());
                    assert!(*leaf_idx < stack_base.len());

                    let (leaf_pos, path) = &stack_merkle_path[*path_idx];
                    let leaf = &stack_base[*leaf_idx];

                    let path = MerklePath {
                        chip_1: config.merkle_chip_1(),
                        chip_2: config.merkle_chip_2(),
                        domain: OrchardHashDomains::MerkleCrh,
                        leaf_pos: *leaf_pos,
                        path: *path,
                    };

                    let root =
                        path.calculate_root(layouter.namespace(|| "calculate root"), *leaf)?;
                    stack_base.push(root);
                }
            }
        }

        // At this point we've enforced all of our public inputs.
        Ok(())
    }
}
