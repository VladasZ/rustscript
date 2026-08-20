//! Windows services, backed by the windows-service crate.
//!
//! The script side mirrors the real crate. `ServiceManager::local_computer`
//! opens the manager, `open_service` opens one service, and the service answers
//! `query_status`, `query_config`, `change_config`, `start` and `stop`. An
//! earlier version of this bridge invented a `WindowsService` type that no crate
//! exports, which meant a script using it could never pass the `cargo check`
//! gate. Bridges expose the real API.
//!
//! Neither handle is kept alive. The manager value carries the access mask it
//! was opened with and the service value carries its name and mask, and the real
//! handles are reopened per call. That is how the registry bridge works too, and
//! it keeps the `Native` enum free of cfg.
//!
//! On a non-Windows host every call returns a plain error saying so.

use anyhow::Result;

use super::bytecode::{MethodName, PathId};
use super::std_bridge::as_i64;
use super::value::{StructData, Value};

/// The access mask constants, as plain ints. `ServiceAccess` and
/// `ServiceManagerAccess` are bitflags in the crate, so `|` on the script side
/// works on these directly.
pub(super) fn service_const(id: PathId) -> Option<Value> {
    let n: i64 = match id {
        // ServiceManagerAccess and ServiceAccess share the low bit values.
        PathId::Connect | PathId::QueryConfig => 0x0001,
        PathId::CreateService | PathId::ChangeConfig => 0x0002,
        PathId::EnumerateService | PathId::QueryStatus => 0x0004,
        PathId::Start => 0x0010,
        PathId::Stop => 0x0020,
        PathId::Delete => 0x0001_0000,
        _ => return None,
    };
    Some(Value::Int(n))
}

/// `ServiceManager::local_computer(database, access)`. The database argument is
/// accepted and ignored, matching the crate where `None` is the only value the
/// setup scripts ever pass.
pub(super) fn local_computer(args: &[Value]) -> Value {
    let access = args.get(1).and_then(as_i64).unwrap_or(0x0001);
    Value::ok(Value::struct_of(
        "ServiceManager",
        [("access".into(), Value::Int(access))],
    ))
}

pub(super) fn manager_method(s: &StructData, name: &MethodName, args: &[Value]) -> Result<Value> {
    imp::manager_method(s, name, args)
}

pub(super) fn service_method(s: &StructData, name: &MethodName, args: &[Value]) -> Result<Value> {
    imp::service_method(s, name, args)
}

#[cfg(windows)]
mod imp {
    use super::super::bytecode::{BuiltinId, MethodName};
    use anyhow::{Result, bail};
    use windows_service::service::{
        Service, ServiceAccess, ServiceDependency, ServiceErrorControl, ServiceInfo,
        ServiceStartType, ServiceState, ServiceType,
    };
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    use std::sync::Arc;

    use super::super::enum_def::{EnumDef, SERVICE_START_TYPE, SERVICE_STATE};
    use super::super::std_bridge::as_i64;
    use super::super::value::{StructData, Value};

    /// A `ServiceState` or `ServiceStartType` variant, mirrored as an enum
    /// value so `{:?}` prints the bare variant name the way the compiled
    /// crate does.
    fn service_variant(def: &Arc<EnumDef>, name: &str) -> Option<Value> {
        Value::enum_named(def, name, Vec::new())
    }

    fn field_str(s: &StructData, name: &str) -> String {
        s.get(name).map(|v| v.display()).unwrap_or_default()
    }

    fn field_i64(s: &StructData, name: &str) -> i64 {
        s.get(name).as_ref().and_then(as_i64).unwrap_or_default()
    }

    /// Open the manager with the mask the script asked `local_computer` for.
    fn manager(access: i64) -> Result<ServiceManager> {
        let mask = ServiceManagerAccess::from_bits_truncate(access as u32);
        Ok(ServiceManager::local_computer(None::<&str>, mask)?)
    }

    /// Reopen a service from the name and masks its value carries. The manager
    /// mask travels with the service so a reopen matches the original request.
    fn open(s: &StructData) -> Result<Service> {
        let access = ServiceAccess::from_bits_truncate(field_i64(s, "access") as u32);
        Ok(manager(field_i64(s, "manager_access"))?.open_service(field_str(s, "name"), access)?)
    }

    fn state_name(state: ServiceState) -> &'static str {
        match state {
            ServiceState::Stopped => "Stopped",
            ServiceState::StartPending => "StartPending",
            ServiceState::StopPending => "StopPending",
            ServiceState::Running => "Running",
            ServiceState::ContinuePending => "ContinuePending",
            ServiceState::PausePending => "PausePending",
            ServiceState::Paused => "Paused",
        }
    }

    fn start_type_name(t: ServiceStartType) -> &'static str {
        match t {
            ServiceStartType::AutoStart => "AutoStart",
            ServiceStartType::OnDemand => "OnDemand",
            ServiceStartType::Disabled => "Disabled",
            ServiceStartType::BootStart => "BootStart",
            ServiceStartType::SystemStart => "SystemStart",
        }
    }

    fn start_type_from(v: &Value) -> Result<ServiceStartType> {
        let name = match v {
            Value::Enum { def, variant, .. } => def.variant_name(*variant).to_string(),
            other => other.display(),
        };
        Ok(match name.as_str() {
            "AutoStart" => ServiceStartType::AutoStart,
            "OnDemand" => ServiceStartType::OnDemand,
            "Disabled" => ServiceStartType::Disabled,
            "BootStart" => ServiceStartType::BootStart,
            "SystemStart" => ServiceStartType::SystemStart,
            other => bail!("unknown service start type `{other}`"),
        })
    }

    fn error_control_raw(e: ServiceErrorControl) -> i64 {
        match e {
            ServiceErrorControl::Ignore => 0,
            ServiceErrorControl::Normal => 1,
            ServiceErrorControl::Severe => 2,
            ServiceErrorControl::Critical => 3,
        }
    }

    fn error_control_from(n: i64) -> ServiceErrorControl {
        match n {
            0 => ServiceErrorControl::Ignore,
            2 => ServiceErrorControl::Severe,
            3 => ServiceErrorControl::Critical,
            _ => ServiceErrorControl::Normal,
        }
    }

    fn dependency_name(d: &ServiceDependency) -> String {
        match d {
            ServiceDependency::Service(n) => n.to_string_lossy().into_owned(),
            ServiceDependency::Group(n) => format!("+{}", n.to_string_lossy()),
        }
    }

    fn dependency_from(name: &str) -> ServiceDependency {
        // Windows marks a load order group with a leading plus, and the crate
        // splits that into its own variant, so the prefix decides which one.
        match name.strip_prefix('+') {
            Some(group) => ServiceDependency::Group(group.into()),
            None => ServiceDependency::Service(name.into()),
        }
    }

    fn strings(v: Option<Value>) -> Vec<String> {
        match v {
            Some(Value::Vec(items)) => items.lock().iter().map(Value::display).collect(),
            _ => Vec::new(),
        }
    }

    /// Rebuild the real `ServiceInfo` from the value a script passed in. Every
    /// field is read, so nothing the script set is dropped on the way through.
    fn service_info_from(info: &StructData) -> Result<ServiceInfo> {
        let Some(start) = info.get("start_type") else {
            bail!("a ServiceInfo needs a start_type");
        };
        let account = info
            .get("account_name")
            .and_then(|v| v.some_payload())
            .map(|v| v.display().into());
        let password = info
            .get("account_password")
            .and_then(|v| v.some_payload())
            .map(|v| v.display().into());
        Ok(ServiceInfo {
            name: field_str(info, "name").into(),
            display_name: field_str(info, "display_name").into(),
            service_type: ServiceType::from_bits_truncate(field_i64(info, "service_type") as u32),
            start_type: start_type_from(&start)?,
            error_control: error_control_from(field_i64(info, "error_control")),
            executable_path: field_str(info, "executable_path").into(),
            launch_arguments: strings(info.get("launch_arguments"))
                .into_iter()
                .map(Into::into)
                .collect(),
            dependencies: strings(info.get("dependencies"))
                .iter()
                .map(|d| dependency_from(d))
                .collect(),
            account_name: account,
            account_password: password,
        })
    }

    fn io_result(r: Result<()>) -> Value {
        match r {
            Ok(()) => Value::ok(Value::Unit),
            Err(e) => Value::err(Value::str(e.to_string())),
        }
    }

    pub(super) fn manager_method(
        s: &StructData,
        name: &MethodName,
        args: &[Value],
    ) -> Result<Value> {
        Ok(match name.id {
            BuiltinId::OpenService => {
                let svc_name = args.first().map(Value::display).unwrap_or_default();
                let access = args.get(1).and_then(as_i64).unwrap_or(0x0004);
                let value = Value::struct_of(
                    "Service",
                    [
                        ("name".into(), Value::str(svc_name)),
                        ("access".into(), Value::Int(access)),
                        ("manager_access".into(), Value::Int(field_i64(s, "access"))),
                    ],
                );
                // Open it once now so a missing service reports here, the way
                // the real call does, instead of on the first query.
                let Value::Struct(probe) = &value else {
                    bail!("could not build the service value");
                };
                match open(probe) {
                    Ok(_) => Value::ok(value.clone()),
                    Err(e) => Value::err(Value::str(e.to_string())),
                }
            }
            _ => bail!("unknown method `{name}` on ServiceManager"),
        })
    }

    pub(super) fn service_method(
        s: &StructData,
        name: &MethodName,
        args: &[Value],
    ) -> Result<Value> {
        Ok(match name.id {
            BuiltinId::QueryStatus => match open(s).and_then(|svc| Ok(svc.query_status()?)) {
                Ok(st) => Value::ok(Value::struct_of(
                    "ServiceStatus",
                    [(
                        "current_state".into(),
                        service_variant(&SERVICE_STATE, state_name(st.current_state))
                            .unwrap_or(Value::Unit),
                    )],
                )),
                Err(e) => Value::err(Value::str(e.to_string())),
            },
            // Every field comes back, not just the ones a script usually
            // reads. change_config needs a complete ServiceInfo, so anything
            // missing here would have to be invented there, behind the script's
            // back. service_type and error_control are handed over as their raw
            // values so they round trip exactly without the bridge having to
            // model every variant.
            BuiltinId::QueryConfig => match open(s).and_then(|svc| Ok(svc.query_config()?)) {
                Ok(cfg) => Value::ok(Value::struct_of(
                    "ServiceConfig",
                    [
                        (
                            "service_type".into(),
                            Value::Int(i64::from(cfg.service_type.bits())),
                        ),
                        (
                            "start_type".into(),
                            service_variant(&SERVICE_START_TYPE, start_type_name(cfg.start_type))
                                .unwrap_or(Value::Unit),
                        ),
                        (
                            "error_control".into(),
                            Value::Int(error_control_raw(cfg.error_control)),
                        ),
                        (
                            "executable_path".into(),
                            Value::str(cfg.executable_path.display().to_string()),
                        ),
                        (
                            "display_name".into(),
                            Value::str(cfg.display_name.to_string_lossy().into_owned()),
                        ),
                        (
                            "account_name".into(),
                            match cfg.account_name {
                                Some(a) => {
                                    Value::some(Value::str(a.to_string_lossy().into_owned()))
                                }
                                None => Value::none(),
                            },
                        ),
                        (
                            "dependencies".into(),
                            Value::vec(
                                cfg.dependencies
                                    .iter()
                                    .map(|d| Value::str(dependency_name(d)))
                                    .collect(),
                            ),
                        ),
                    ],
                )),
                Err(e) => Value::err(Value::str(e.to_string())),
            },
            // ChangeServiceConfigW rewrites the whole service record, so the
            // script hands over a complete ServiceInfo and every field of it is
            // used. Nothing is silently substituted from the current config,
            // because a script that set executable_path and had it quietly
            // dropped would be the worst kind of bug to find later.
            BuiltinId::ChangeConfig => {
                let Some(Value::Struct(info)) = args.first() else {
                    bail!("change_config takes a ServiceInfo");
                };
                io_result(open(s).and_then(|svc| {
                    svc.change_config(&service_info_from(info)?)?;
                    Ok(())
                }))
            }
            BuiltinId::Start => io_result(open(s).and_then(|svc| {
                svc.start(&[] as &[&std::ffi::OsStr])?;
                Ok(())
            })),
            BuiltinId::Stop => io_result(open(s).and_then(|svc| {
                svc.stop()?;
                Ok(())
            })),
            _ => bail!("unknown method `{name}` on Service"),
        })
    }
}

#[cfg(not(windows))]
mod imp {
    use super::super::bytecode::MethodName;
    use anyhow::{Result, bail};

    use super::super::value::{StructData, Value};

    pub(super) fn manager_method(
        _s: &StructData,
        name: &MethodName,
        _args: &[Value],
    ) -> Result<Value> {
        bail!("ServiceManager::{name} is a windows service, it does not exist on this platform")
    }

    pub(super) fn service_method(
        _s: &StructData,
        name: &MethodName,
        _args: &[Value],
    ) -> Result<Value> {
        bail!("Service::{name} is a windows service, it does not exist on this platform")
    }
}
