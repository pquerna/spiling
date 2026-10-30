// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use spiling_contracts::{OperationView, operation_view};
use spiling_engine_client::{ClientError, EngineRpc};
pub async fn observe(rpc: EngineRpc, name: String) -> Result<(OperationView, bool), ClientError> {
    let operation = rpc.get_operation(&name).await?;
    let view = operation_view(&operation).map_err(ClientError::Protocol)?;
    if view.name != name {
        return Err(ClientError::Protocol(
            "Diagnostic operation identity changed".into(),
        ));
    }
    let mut triangle = false;
    if view.done && view.error_code.is_none() {
        for output in &view.outputs {
            let descriptor = rpc.get_artifact(&output.name).await?;
            if spiling_contracts::ArtifactView::from(&descriptor) != *output
                || descriptor.size_bytes != 64
            {
                return Err(ClientError::Protocol(
                    "Diagnostic descriptor changed or exceeds triangle size".into(),
                ));
            }
            let bytes = rpc.read_artifact(&descriptor).await?;
            if bytes != spiling_contracts::synthetic_triangle() {
                return Err(ClientError::Protocol(
                    "Diagnostic did not return the canonical unitless SPLT triangle".into(),
                ));
            }
            triangle = true;
        }
    }
    Ok((view, triangle))
}
