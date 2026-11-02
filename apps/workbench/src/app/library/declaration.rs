// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use spiling_contracts::library::FilamentDeclaration;
#[derive(Default)]
pub(super) struct DeclarationDraft {
    pub brand: String,
    pub material: String,
    pub diameter: String,
    pub td: String,
}
impl DeclarationDraft {
    pub fn from(d: &FilamentDeclaration) -> Self {
        Self {
            brand: d.brand.clone().unwrap_or_default(),
            material: d.material_label.clone().unwrap_or_default(),
            diameter: d.diameter_mm.map_or(String::new(), |v| v.to_string()),
            td: d.hueforge_td_mm.map_or(String::new(), |v| v.to_string()),
        }
    }
    pub fn value(&self) -> Result<FilamentDeclaration, String> {
        fn text(v: &str) -> Option<String> {
            let v = v.trim();
            (!v.is_empty()).then(|| v.to_owned())
        }
        fn number(v: &str, label: &str) -> Result<Option<f64>, String> {
            let v = v.trim();
            if v.is_empty() {
                Ok(None)
            } else {
                v.parse::<f64>()
                    .map(Some)
                    .map_err(|_| format!("Enter {label} as a number, or leave it unknown"))
            }
        }
        let v = FilamentDeclaration {
            brand: text(&self.brand),
            material_label: text(&self.material),
            diameter_mm: number(&self.diameter, "diameter")?,
            hueforge_td_mm: number(&self.td, "HueForge TD")?,
        };
        v.validate()?;
        Ok(v)
    }
    pub fn differs(&self, d: &FilamentDeclaration) -> bool {
        self.value().as_ref() != Ok(d)
    }
}
