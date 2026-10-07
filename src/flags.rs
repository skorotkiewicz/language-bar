use base64::{Engine, engine::general_purpose::STANDARD};
use ksni::Icon;
use resvg::{tiny_skia, usvg};
use std::{collections::HashMap, fs, sync::OnceLock};

fn registry(xml: &str) -> Result<HashMap<String, String>, roxmltree::Error> {
    let doc = roxmltree::Document::parse_with_options(
        xml,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            nodes_limit: 100_000,
            ..Default::default()
        },
    )?;
    let mut names = HashMap::new();
    for layout in doc.descendants().filter(|n| n.has_tag_name("layout")) {
        let Some(item) = layout.children().find(|n| n.has_tag_name("configItem")) else {
            continue;
        };
        let text = |tag| {
            item.children()
                .find(|n| n.has_tag_name(tag))
                .and_then(|n| n.text())
        };
        let code = text("name").unwrap_or_default().to_uppercase();
        // Layout codes usually identify a country. For language-only layouts, use a
        // flag only when the registry identifies exactly one country, never guess.
        let countries: Vec<_> = item
            .descendants()
            .filter(|n| n.has_tag_name("iso3166Id"))
            .filter_map(|n| n.text())
            .collect();
        let country = if rs_grid_icons::flag_data_uri(&code).is_some() {
            code
        } else if countries.len() == 1 {
            countries[0].into()
        } else {
            continue;
        };
        for config in layout
            .descendants()
            .filter(|n| n.has_tag_name("configItem"))
        {
            if let Some(description) = config
                .children()
                .find(|n| n.has_tag_name("description"))
                .and_then(|n| n.text())
            {
                names.insert(description.to_owned(), country.clone());
            }
        }
        if let Some(code) = text("name") {
            names.insert(code.into(), country);
        }
    }
    Ok(names)
}

fn countries() -> &'static HashMap<String, String> {
    static COUNTRIES: OnceLock<HashMap<String, String>> = OnceLock::new();
    COUNTRIES.get_or_init(|| {
        let dir = std::env::var_os("XKB_CONFIG_ROOT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| "/usr/share/X11/xkb".into());
        let path = dir.join("rules/evdev.xml");
        match fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|xml| registry(&xml).map_err(|e| e.to_string()))
        {
            Ok(map) => map,
            Err(e) => {
                eprintln!(
                    "Could not load XKB flag mappings from {}: {e}",
                    path.display()
                );
                HashMap::new()
            }
        }
    })
}

fn render(svg: &[u8]) -> Result<Icon, Box<dyn std::error::Error>> {
    let tree = usvg::Tree::from_data(svg, &usvg::Options::default())?;
    let mut pixmap = tiny_skia::Pixmap::new(32, 32).ok_or("Could not allocate flag image")?;
    let scale = (32.0 / tree.size().width()).min(32.0 / tree.size().height());
    let transform = tiny_skia::Transform::from_scale(scale, scale).post_translate(
        (32.0 - tree.size().width() * scale) / 2.0,
        (32.0 - tree.size().height() * scale) / 2.0,
    );
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    // tiny-skia stores premultiplied RGBA; StatusNotifier expects straight ARGB.
    let data = pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let p = p.demultiply();
            [p.alpha(), p.red(), p.green(), p.blue()]
        })
        .collect();
    Ok(Icon {
        width: 32,
        height: 32,
        data,
    })
}

pub fn icon(layout: &str) -> Icon {
    let result = countries()
        .get(layout)
        .or_else(|| countries().get(&layout.to_lowercase()))
        .and_then(|code| rs_grid_icons::flag_data_uri(code))
        .and_then(|uri| uri.strip_prefix("data:image/svg+xml;base64,"))
        .and_then(|encoded| STANDARD.decode(encoded).ok())
        .and_then(|svg| render(&svg).ok());
    result.unwrap_or_else(|| render(br##"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="24"><rect x="1" y="2" width="30" height="20" rx="2" fill="#555"/><path d="M5 7h3m3 0h3m3 0h3m3 0h3M5 12h3m3 0h3m3 0h3m3 0h3M9 17h14" stroke="white" stroke-width="2"/></svg>"##).expect("built-in keyboard SVG is valid"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_bundled_flags_and_maps_layout_variants() {
        assert!(rs_grid_icons::flag_count() >= 250);
        for (_, uri) in rs_grid_icons::all_flags() {
            let svg = STANDARD.decode(uri.split_once(',').unwrap().1).unwrap();
            let image = render(&svg).unwrap();
            assert_eq!(image.data.len(), 32 * 32 * 4);
            assert!(image.data.chunks_exact(4).any(|p| p[0] > 0));
        }
        let xml = r#"<xkbConfigRegistry><layoutList><layout><configItem><name>jp</name><description>Japanese</description></configItem><variantList><variant><configItem><name>kana</name><description>Japanese (Kana)</description></configItem></variant></variantList></layout><layout><configItem><name>ara</name><description>Arabic</description><countryList><iso3166Id>AE</iso3166Id><iso3166Id>EG</iso3166Id></countryList></configItem></layout></layoutList></xkbConfigRegistry>"#;
        let map = registry(xml).unwrap();
        assert_eq!(map.get("Japanese (Kana)").unwrap(), "JP");
        assert!(!map.contains_key("Arabic"));
        assert_ne!(icon("Japanese").data, icon("Polish").data);
        assert_ne!(icon("English (UK)").data, icon("English (US)").data);
    }
}
