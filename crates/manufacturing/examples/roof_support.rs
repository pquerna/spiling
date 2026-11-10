use serde_json::{Value, json};
use spiling_contracts::manufacturing::{
    DepositionKind, DepositionOwner, ManufacturingIntent, NormalizedPrintPlan,
    program_coordinates::{project_xyz6_coordinate_mm, project_xyz6_point_mm},
};
use std::{
    fs::File,
    io::{BufReader, Write},
    sync::atomic::AtomicBool,
};

#[path = "roof_support/policy.rs"]
mod policy;

fn read(path: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let file = File::open(path)?;
    if file.metadata()?.len() > 256 * 1024 * 1024 {
        return Err("research input exceeds byte bound".into());
    }
    Ok(serde_json::from_reader(BufReader::new(file))?)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 5 || !matches!(args[1].as_str(), "classify" | "select") {
        return Err(
            "expected classify PLAN INTENT NEW_OUTPUT or select CONTEXT POLICY NEW_OUTPUT".into(),
        );
    }
    if args[1] == "select" {
        let result = policy::select(&read(&args[2])?, &read(&args[3])?);
        let mut file = File::options()
            .create_new(true)
            .write(true)
            .open(&args[4])?;
        serde_json::to_writer(&mut file, &result)?;
        println!(
            "{}",
            json!({"attempted":result["attempted"],"removed":result["removed"],"interval_checks":result["interval_checks"],"selector_s":result["selector_s"]})
        );
        return Ok(());
    }
    let plan: NormalizedPrintPlan = serde_json::from_value(read(&args[2])?)?;
    let intent: ManufacturingIntent = serde_json::from_value(read(&args[3])?)?;
    let columns = spiling_manufacturing::planner::cathedral_research_columns(
        &plan,
        &intent.recipe,
        &AtomicBool::new(false),
    )?
    .ok_or("missing automatic Cathedral column inventory")?;
    let project = project_xyz6_coordinate_mm;
    let mut optional = Vec::new();
    for layer in &plan.layers {
        for (path_index, path) in layer.paths.iter().enumerate() {
            if path.kind != DepositionKind::SparseFill
                || !matches!(path.owner, Some(DepositionOwner::Regional { .. }))
                || path.points_mm.len() != 2
            {
                continue;
            }
            let [start, stop] = [path.points_mm[0], path.points_mm[1]];
            let fixed = usize::from(start[0] != stop[0]);
            if start[fixed] != stop[fixed] {
                continue;
            }
            let coordinate = project(start[fixed])?;
            let mut protected = false;
            for root in &columns.roots[fixed] {
                protected |= project(*root)? == coordinate;
            }
            let along = 1 - fixed;
            let extent = [start[along].min(stop[along]), start[along].max(stop[along])];
            for &(axis, column, low, high) in &columns.joints {
                protected |= axis == fixed
                    && project(column)? == coordinate
                    && extent[0] <= high
                    && low <= extent[1];
            }
            if !protected {
                optional
                    .push(json!({"layer":layer.index,"path":path_index,"start":project_xyz6_point_mm(start)?,"end":project_xyz6_point_mm(stop)?}));
            }
        }
    }
    let result = json!({"schema":1,"scope":"research_subset_inventory_not_manufacturing_authority",
        "root_counts":columns.roots.each_ref().map(Vec::len),"joint_columns":columns.joints.len(),
        "optional":optional});
    let mut file = File::options()
        .create_new(true)
        .write(true)
        .open(&args[4])?;
    serde_json::to_writer(&mut file, &result)?;
    file.write_all(b"\n")?;
    Ok(())
}
