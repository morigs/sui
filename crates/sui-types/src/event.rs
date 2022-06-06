// Copyright (c) 2022, Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use move_bytecode_utils::{layout::TypeLayoutBuilder, module_cache::GetModule};
use move_core_types::value::MoveStructLayout;
use move_core_types::{
    language_storage::{ModuleId, StructTag, TypeTag},
    value::{MoveStruct, MoveTypeLayout},
};
use name_variant::NamedVariant;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_with::{serde_as, Bytes};
use strum_macros::EnumDiscriminants;

use crate::object::ObjectFormatOptions;
use crate::{
    base_types::{ObjectID, SequenceNumber, SuiAddress, TransactionDigest},
    committee::EpochId,
    error::SuiError,
    messages_checkpoint::CheckpointSequenceNumber,
};
use schemars::JsonSchema;

/// A universal Sui event type encapsulating different types of events
#[derive(Debug, Clone, PartialEq)]
pub struct EventEnvelope {
    /// UTC timestamp in milliseconds since epoch (1/1/1970)
    timestamp: u64,
    /// Transaction digest of associated transaction, if any
    tx_digest: Option<TransactionDigest>,
    /// Specific event type
    pub event: Event,
    /// json value for MoveStruct (for MoveEvent only)
    pub move_struct_json_value: Option<Value>,
}

impl EventEnvelope {
    pub fn new(
        timestamp: u64,
        tx_digest: Option<TransactionDigest>,
        event: Event,
        move_struct_json_value: Option<Value>,
    ) -> Self {
        Self {
            timestamp,
            tx_digest,
            event,
            move_struct_json_value,
        }
    }

    pub fn event_type(&self) -> &'static str {
        self.event.variant_name()
    }
}

#[derive(Eq, Debug, Clone, PartialEq, Deserialize, Serialize, Hash, JsonSchema)]
pub enum TransferType {
    Coin,
    ToAddress,
    ToObject, // wrap object in another object
}

/// Specific type of event
#[serde_as]
#[derive(
    Eq, Debug, Clone, PartialEq, NamedVariant, Deserialize, Serialize, Hash, EnumDiscriminants,
)]
#[strum_discriminants(name(EventType))]
pub enum Event {
    /// Move-specific event
    MoveEvent(MoveEvent),
    /// Module published
    Publish { package_id: ObjectID },
    /// Transfer objects to new address / wrap in another object / coin
    TransferObject {
        object_id: ObjectID,
        version: SequenceNumber,
        destination_addr: SuiAddress,
        type_: TransferType,
    },
    /// Delete object
    DeleteObject(ObjectID),
    /// New object creation
    NewObject(ObjectID),
    /// Epooch change
    EpochChange(EpochId),
    /// New checkpoint
    Checkpoint(CheckpointSequenceNumber),
}

impl Event {
    pub fn move_event(type_: StructTag, contents: Vec<u8>) -> Self {
        Event::MoveEvent(MoveEvent { type_, contents })
    }

    /// Returns the EventType associated with an Event
    pub fn event_type(&self) -> EventType {
        self.into()
    }

    /// Returns the object or package ID associated with the event, if available.  Specifically:
    /// - For TransferObject: the object ID being transferred (eg moving child from parent, its the child)
    /// - for Publish, the package ID (which is the object ID of the module)
    /// - for DeleteObject and NewObject, the Object ID
    pub fn object_id(&self) -> Option<ObjectID> {
        match self {
            Event::Publish { package_id } => Some(*package_id),
            Event::TransferObject { object_id, .. } => Some(*object_id),
            Event::DeleteObject(obj_id) => Some(*obj_id),
            Event::NewObject(obj_id) => Some(*obj_id),
            _ => None,
        }
    }

    /// Extract a module ID, if available, from a SuiEvent
    pub fn module_id(&self) -> Option<ModuleId> {
        match self {
            Event::MoveEvent(event) => Some(event.type_.module_id()),
            _ => None,
        }
    }

    /// Extracts a MoveStruct, if possible, from the event
    pub fn extract_move_struct(
        &self,
        resolver: &impl GetModule,
    ) -> Result<Option<MoveStruct>, SuiError> {
        match self {
            Event::MoveEvent(event) => {
                let typestruct = TypeTag::Struct(event.type_.clone());
                let layout =
                    TypeLayoutBuilder::build_with_fields(&typestruct, resolver).map_err(|e| {
                        SuiError::ObjectSerializationError {
                            error: e.to_string(),
                        }
                    })?;
                match layout {
                    MoveTypeLayout::Struct(l) => {
                        let s =
                            MoveStruct::simple_deserialize(&event.contents, &l).map_err(|e| {
                                SuiError::ObjectSerializationError {
                                    error: e.to_string(),
                                }
                            })?;
                        Ok(Some(s))
                    }
                    _ => unreachable!(
                        "We called build_with_types on Struct type, should get a struct layout"
                    ),
                }
            }
            _ => Ok(None),
        }
    }
}

#[serde_as]
#[derive(Eq, Debug, Clone, PartialEq, Deserialize, Serialize, Hash)]
pub struct MoveEvent {
    pub type_: StructTag,
    #[serde_as(as = "Bytes")]
    pub contents: Vec<u8>,
}

impl MoveEvent {
    /// Get a `MoveStructLayout` for `self`.
    /// The `resolver` value must contain the module that declares `self.type_` and the (transitive)
    /// dependencies of `self.type_` in order for this to succeed. Failure will result in an `ObjectSerializationError`
    pub fn get_layout(
        &self,
        format: ObjectFormatOptions,
        resolver: &impl GetModule,
    ) -> Result<MoveStructLayout, SuiError> {
        let type_ = TypeTag::Struct(self.type_.clone());
        let layout = if format.include_types {
            TypeLayoutBuilder::build_with_types(&type_, resolver)
        } else {
            TypeLayoutBuilder::build_with_fields(&type_, resolver)
        }
        .map_err(|e| SuiError::ObjectSerializationError {
            error: e.to_string(),
        })?;
        match layout {
            MoveTypeLayout::Struct(l) => Ok(l),
            _ => unreachable!(
                "We called build_with_types on Struct type, should get a struct layout"
            ),
        }
    }
    /// Convert `self` to the JSON representation dictated by `layout`.
    pub fn to_move_struct(&self, layout: &MoveStructLayout) -> Result<MoveStruct, SuiError> {
        MoveStruct::simple_deserialize(&self.contents, layout).map_err(|e| {
            SuiError::ObjectSerializationError {
                error: e.to_string(),
            }
        })
    }

    /// Convert `self` to the JSON representation dictated by `layout`.
    pub fn to_move_struct_with_resolver(
        &self,
        format: ObjectFormatOptions,
        resolver: &impl GetModule,
    ) -> Result<MoveStruct, SuiError> {
        self.to_move_struct(&self.get_layout(format, resolver)?)
    }
}
