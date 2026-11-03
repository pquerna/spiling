// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Ordered layout client operations. Keep their async state out of the workflow dispatcher.
use super::*;
pub(super) async fn prepare(
    client: &mut EngineClient,
    request: LayoutRequest,
) -> Result<LayoutProposal, Error> {
    let session = client.hello().session_id.clone();
    let captured = info(client).await?;
    let scene = geometry::scene(client, &session).await?;
    let response = geometry::control(
        client,
        GeometryCommand::PrepareLayout {
            session_id: session.clone(),
            base_revision: scene.revision,
            project_revision: captured.revision,
            request,
        },
    )
    .await?;
    let GeometryResponse::OperationAccepted { operation } = response else {
        return Err("expected accepted layout search".into());
    };
    let JobResult::LayoutPrepared { proposal } =
        geometry::wait_operation(client, &session, operation).await?
    else {
        return Err("expected checked layout proposal".into());
    };
    proposal.validate()?;
    if proposal.project_id != captured.project_id
        || proposal.project_revision != captured.revision
        || proposal.scene_revision != scene.revision
    {
        return Err("layout capture mismatch".into());
    }
    Ok(proposal)
}
pub(super) async fn apply(
    client: &mut EngineClient,
    proposal: &LayoutProposal,
) -> Result<(), Error> {
    let captured = info(client).await?;
    if proposal.project_id != captured.project_id || proposal.project_revision != captured.revision
    {
        return Err("layout proposal is stale; search again".into());
    }
    let response = geometry::control(
        client,
        GeometryCommand::ApplyLayout {
            session_id: client.hello().session_id.clone(),
            base_revision: proposal.scene_revision,
            project_revision: proposal.project_revision,
            proposal_id: proposal.id.clone(),
        },
    )
    .await?;
    if !matches!(response, GeometryResponse::SceneChanged { .. }) {
        return Err("unexpected layout application response".into());
    }
    Ok(())
}
