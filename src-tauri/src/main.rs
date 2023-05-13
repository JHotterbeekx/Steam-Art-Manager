#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod reader;
mod writer;
mod vdf_structs;
mod logger;
mod steam;
mod zip_controller;
mod appinfo_vdf_parser;
mod shortcuts_vdf_parser;
mod vdf_reader;

use std::{path::PathBuf, collections::HashMap, fs::{self, File}, io::Write};

use appinfo_vdf_parser::open_appinfo_vdf;
use serde_json::{Map, Value};
use shortcuts_vdf_parser::{open_shortcuts_vdf, write_shortcuts_vdf};

use home::home_dir;

use serde;
use reqwest;
use steam::get_steam_root_dir;
use tauri::{
  AppHandle,
  api::dialog::blocking::FileDialogBuilder,
  FsScope, Manager
};
use keyvalues_parser::Vdf;

#[derive(Clone, serde::Serialize)]
struct Payload {
  args: Vec<String>,
  cwd: String,
}

type GridImageCache = HashMap<String, HashMap<String, String>>;

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq, Clone)]
#[allow(non_snake_case)]
struct ChangedPath {
  appId: String,
  gridType: String,
  oldPath: String,
  targetPath: String,
  sourcePath: String
}

/// Gets a grid's file name based on its type.
fn get_grid_filename(app_handle: &AppHandle, appid: &str, grid_type: &str, image_type: &str) -> String {
  match grid_type {
    "Capsule" => return format!("{}p{}", appid, image_type),
    "Wide Capsule" => return format!("{}{}", appid, image_type),
    "Hero" => return format!("{}_hero{}", appid, image_type),
    "Logo" => return format!("{}_logo{}", appid, image_type),
    "Icon" => return format!("{}_icon{}", appid, image_type),
    _ => {
      logger::log_to_file(app_handle.to_owned(), format!("Unexpected grid type {}", grid_type).as_str(), 2);
      panic!("Unexpected grid type {}", grid_type);
    }
  }
}

/// Adjusts the path of a grid based on its type.
fn adjust_path(app_handle: &AppHandle, appid: &str, path: &str, grid_type: &str) -> String {
  let format_start_index = path.rfind(".").expect("Path should have had a file extension.");
  let image_type = &path[format_start_index..];
  return get_grid_filename(app_handle, appid, grid_type, image_type);
}

/// Filters the grid paths based on which have change.
fn filter_paths(app_handle: &AppHandle, steam_active_user_id: String, current_paths: &GridImageCache, original_paths: &GridImageCache) -> Vec<ChangedPath> {
  let grids_dir = PathBuf::from(steam::get_grids_directory(app_handle.to_owned(), steam_active_user_id));
  let mut res:Vec<ChangedPath> = Vec::new();

  for (appid, grids_map) in current_paths.into_iter() {
    for (grid_type, source_path) in grids_map.into_iter() {
      let mut grid_path: &String = &String::from("");

      if original_paths.get(appid.as_str()).is_some() && original_paths.get(appid.as_str()).unwrap().get(grid_type.as_str()).is_some() {
        grid_path = original_paths.get(appid.as_str()).unwrap().get(grid_type.as_str()).unwrap();
      }

      let grid_path_owned = grid_path.to_owned();
      let source_path_owned = source_path.to_owned();

      if source_path_owned != grid_path_owned {
        let target_path;

        if source_path != "REMOVE" {
          let adjusted_path = adjust_path(app_handle, appid.as_str(), source_path_owned.as_str(), grid_type.as_str()).replace("\\", "/");
          target_path = String::from(grids_dir.join(adjusted_path).to_str().unwrap()).replace("\\", "/");
        } else {
          target_path = String::from("REMOVE");
        }

        let mut changed_path = ChangedPath {
          appId: appid.to_owned(),
          gridType: grid_type.to_owned(),
          oldPath: grid_path_owned.replace("\\", "/"),
          targetPath: target_path.to_owned(),
          sourcePath: source_path_owned.replace("\\", "/")
        };

        if changed_path.targetPath.ends_with(".webp") {
          let target: String = changed_path.targetPath;
          let mut jpg_target: String = target[..target.len() - 5].to_owned();
          jpg_target.push_str(".jpg");

          changed_path.targetPath = String::from(jpg_target.to_owned());
        }

        res.push(changed_path);
      }
    }
  }

  return res;
}

/// Checks for shortcut grid changes.
fn check_for_shortcut_changes(shortcut_icons: &Map<String, Value>, original_shortcut_icons: &Map<String, Value>) -> bool {
  for (shortcut_id, icon) in shortcut_icons.to_owned().into_iter() {
    let icon: &str = icon.as_str().expect("Should have been able to convert icon to &str.");
    let original_icon: &str = original_shortcut_icons.get(&shortcut_id).expect("Original hortcut should have had an icon.").as_str().expect("Should have been able to convert original icon to &str.");

    if icon != original_icon {
      return true;
    }
  }

  return false;
}

#[tauri::command]
/// Exports the users grids to a Grids zip file.
async fn export_grids_to_zip(app_handle: AppHandle, steam_active_user_id: String, platform_id_map: Map<String, Value>, id_name_map: Map<String, Value>) -> bool {
  let file_dialog = FileDialogBuilder::new()
    .set_title("Save Grids Zip")
    .set_file_name("Steam_Grids_Export.zip")
    .add_filter("zip", &["zip"])
    .set_directory(home_dir().expect("Should have been able to get home dir for zip."));

  let file_path = file_dialog.save_file();

  if file_path.is_some() {
    let zip_path = file_path.unwrap();
    logger::log_to_file(app_handle.to_owned(), format!("Got save path: {}", zip_path.to_str().expect("Should have been able to convert path to string.")).as_str(), 0);

    let grids_dir_path = steam::get_grids_directory(app_handle.to_owned(), steam_active_user_id);
    let succeeded = zip_controller::generate_grids_zip(&app_handle, PathBuf::from(grids_dir_path), zip_path, &platform_id_map, &id_name_map);

    if succeeded {
      logger::log_to_file(app_handle.to_owned(), "Successfully saved the user's grids.", 0);
      return true;
    } else {
      logger::log_to_file(app_handle.to_owned(), "Failed to save the user's grids.", 0);
      return false;
    }
  } else {
    logger::log_to_file(app_handle.to_owned(), "No save location was chosen.", 0);
    return false;
  }
}

#[tauri::command]
/// Sets the users grids from a Grids zip file.
async fn import_grids_from_zip(app_handle: AppHandle, steam_active_user_id: String, name_id_map: Map<String, Value>) -> (bool, Map<String, Value>) {
  let file_dialog = FileDialogBuilder::new()
    .set_title("Pick a Grids Zip")
    .add_filter("zip", &["zip"])
    .set_directory(home_dir().expect("Should have been able to get home dir for zip."));

  let file_path = file_dialog.pick_file();

  if file_path.is_some() {
    let zip_path = file_path.unwrap();
    logger::log_to_file(app_handle.to_owned(), format!("Got file path: {}", zip_path.to_str().expect("Should have been able to convert path to string.")).as_str(), 0);

    let grids_dir_path = steam::get_grids_directory(app_handle.to_owned(), steam_active_user_id);
    let (success, icon_map) = zip_controller::set_grids_from_zip(&app_handle, PathBuf::from(grids_dir_path), zip_path, &name_id_map);

    if success {
      logger::log_to_file(app_handle.to_owned(), "Successfully set the user's grids.", 0);
      return (success, icon_map);
    } else {
      logger::log_to_file(app_handle.to_owned(), "Failed to set the user's grids.", 0);
      return (success, icon_map);
    }
  } else {
    logger::log_to_file(app_handle.to_owned(), "No zip file was selected by user.", 0);
    return (false, Map::new());
  }
}

#[tauri::command]
/// Reads the user's appinfo.vdf file.
async fn read_appinfo_vdf(app_handle: AppHandle) -> String {
  let appinfo_path: PathBuf = PathBuf::from(steam::get_appinfo_path(app_handle.to_owned()));
  let appinfo_vdf: Map<String, Value> = open_appinfo_vdf(&appinfo_path);
  return serde_json::to_string(&appinfo_vdf).expect("Should have been able to serialize AppInfo vdf to string.");
}

#[tauri::command]
/// Reads the user's shortcuts.vdf file.
async fn read_shortcuts_vdf(app_handle: AppHandle, steam_active_user_id: String) -> String {
  let shortcuts_path = PathBuf::from(steam::get_shortcuts_path(app_handle.to_owned(), steam_active_user_id));
    
  if shortcuts_path.as_path().exists() {
    logger::log_to_file(app_handle.to_owned(), "shortcuts.vdf exists, reading...", 0);
    let shortcuts_array = open_shortcuts_vdf(&shortcuts_path);
    return serde_json::to_string(&shortcuts_array).expect("Should have been able to serialize Shortcuts vdf to string.");
  } else {
    logger::log_to_file(app_handle.to_owned(), "shortcuts.vdf does not exist.", 0);
    return "{}".to_owned();
  }
}

#[tauri::command]
/// Reads the user's localconfig.vdf file.
async fn read_localconfig_vdf(app_handle: AppHandle, steam_active_user_id: String) -> String {
  let localconfig_path = PathBuf::from(steam::get_localconfig_path(app_handle.to_owned(), steam_active_user_id));
    
  if localconfig_path.as_path().exists() {
    logger::log_to_file(app_handle.to_owned(), "localconfig.vdf exists, reading...", 0);
    let localconfig_contents: String = fs::read_to_string(localconfig_path).expect("localconfig.vdf should have existed.").parse().expect("File should have been a text file.");
    let vdf = Vdf::parse(&localconfig_contents).unwrap();
    let software = vdf.value.get_obj().unwrap().get_key_value("Software").unwrap();
    let valve = software.1[0].get_obj().unwrap().get_key_value("Valve").unwrap();
    let steam = valve.1[0].get_obj().unwrap().get_key_value("Steam").unwrap();
    let apps = steam.1[0].get_obj().unwrap().get_key_value("apps").unwrap();

    let app_entries = &apps.1[0];
    let mut appids: Vec<String> = Vec::new();

    for (_, appid) in app_entries.get_obj().unwrap().keys().enumerate() {
      appids.push(appid.to_string());
    }

    return serde_json::to_string(&appids).expect("Should have been able to serialize localconfig vdf to string.");
  } else {
    logger::log_to_file(app_handle.to_owned(), "localconfig.vdf does not exist.", 0);
    return "{}".to_owned();
  }
}

#[tauri::command]
/// Applies the changes the user has made.
async fn save_changes(app_handle: AppHandle, steam_active_user_id: String, current_art: String, original_art: String, shortcuts_str: String, shortcut_icons: Map<String, Value>, original_shortcut_icons: Map<String, Value>, changed_logo_positions: Map<String, Value>) -> String {
  let current_art_dict: GridImageCache = serde_json::from_str(current_art.as_str()).unwrap();
  let original_art_dict: GridImageCache = serde_json::from_str(original_art.as_str()).unwrap();

  logger::log_to_file(app_handle.to_owned(), "Converting current path entries to grid paths...", 0);
  let paths_to_set: Vec<ChangedPath> = filter_paths(&app_handle, steam_active_user_id.clone(), &current_art_dict, &original_art_dict);
  let paths_id_map: HashMap<String, ChangedPath> = paths_to_set.clone().iter().map(| entry | (format!("{}_{}", entry.appId.to_owned(), entry.gridType.to_owned()).to_string(), entry.to_owned())).collect();
  logger::log_to_file(app_handle.to_owned(), "Current path entries converted to grid paths.", 0);

  for changed_path in (&paths_to_set).into_iter() {
    let source = changed_path.sourcePath.to_owned();
    let target = changed_path.targetPath.to_owned();

    if target == String::from("REMOVE") {
      if changed_path.oldPath.contains("grid") {
        let remove_res = fs::remove_file(changed_path.oldPath.to_owned());
        if remove_res.is_err() {
          let err = remove_res.err().unwrap();
          return format!("{{ \"error\": \"{}\"}}", err.to_string());
        }
        logger::log_to_file(app_handle.to_owned(), format!("Removed grid {}.", changed_path.oldPath.to_owned()).as_str(), 0);
      }
    } else {
      if changed_path.oldPath.contains("grid") {
        let remove_res = fs::remove_file(changed_path.oldPath.to_owned());
        if remove_res.is_err() {
          let err = remove_res.err().unwrap();
          return format!("{{ \"error\": \"{}\"}}", err.to_string());
        }
      }
  
      fs::File::create(target.clone()).unwrap();
      
      let copy_res = fs::copy(source.clone(), target.clone());
  
      if copy_res.is_ok() {
        logger::log_to_file(app_handle.to_owned(), format!("Copied {} to {}.", source, target).as_str(), 0);
      } else {
        logger::log_to_file(app_handle.to_owned(), format!("Failed to copy {} to {}.", source, target).as_str(), 2);
        let err = copy_res.err().unwrap();
        return format!("{{ \"error\": \"{}\"}}", err.to_string());
      }
    }
  }

  let should_change_shortcuts = check_for_shortcut_changes(&shortcut_icons, &original_shortcut_icons);
  
  if should_change_shortcuts {
    logger::log_to_file(app_handle.to_owned(), "Changes to shortcuts detected. Writing shortcuts.vdf...", 0);
    let mut shortcuts_data: Value = serde_json::from_str(shortcuts_str.as_str()).expect("Should have been able to parse json string.");

    let shortcuts_obj_map: &mut Value = shortcuts_data.get_mut("shortcuts").expect("key: shortcuts should have existed.");
    let shortcuts_map: &mut Map<String, Value> = shortcuts_obj_map.as_object_mut().expect("Should have been able to convert shortcuts to map");

    for (_, shortcut) in shortcuts_map.into_iter() {
      let shortcut_map: &mut Map<String, Value> = shortcut.as_object_mut().expect("should have been able to convert shortcut to map.");
      let shortcut_appid_val: &Value = shortcut_map.get("appid").expect("shortcut should have had an appid");
      let shortcut_appid_num: i64 = shortcut_appid_val.as_i64().expect("should have been able to convert shortcut appid to str.");
      let shortcut_appid: String = shortcut_appid_num.to_string();

      let path_key: String = format!("{}_icon", shortcut_appid.to_owned()).to_string();

      if paths_id_map.contains_key(&path_key) {
        let changed_path: &ChangedPath = paths_id_map.get(&path_key).expect("entry should have existed.");
        shortcut_map.insert(String::from("icon"), Value::String(changed_path.targetPath.to_owned()));
      }
    }

    let mut modified_shortcuts_data: Map<String, Value> = Map::new();
    modified_shortcuts_data.insert(String::from("shortcuts"), shortcuts_obj_map.to_owned());
    shortcuts_data = Value::Object(modified_shortcuts_data);

    let shortcuts_vdf_path: PathBuf = PathBuf::from(steam::get_shortcuts_path(app_handle.to_owned(), steam_active_user_id));
    write_shortcuts_vdf(&shortcuts_vdf_path, shortcuts_data);
    logger::log_to_file(app_handle.to_owned(), "Changes to shortcuts saved.", 0);
  } else {
    logger::log_to_file(app_handle.to_owned(), "No changes to shortcuts detected. Skipping...", 0);
  }

  let changed_res = serde_json::to_string::<Vec<ChangedPath>>(paths_to_set.as_ref());

  if changed_res.is_ok() {
    return changed_res.unwrap();
  } else {
    let err = changed_res.err().unwrap();
    logger::log_to_file(app_handle, format!("{}", err.to_string()).as_str(), 2);
    return String::from("[]");
  }
}

#[tauri::command]
/// Writes the user's shortcuts.vdf file.
async fn write_shortcuts(app_handle: AppHandle, steam_active_user_id: String, shortcuts_str: String) -> bool {
  logger::log_to_file(app_handle.to_owned(), "Writing shortcuts.vdf...", 0);
  let shortcuts_vdf_path: PathBuf = PathBuf::from(steam::get_shortcuts_path(app_handle.to_owned(), steam_active_user_id));
  let shortcuts_data: Value = serde_json::from_str(shortcuts_str.as_str()).expect("Should have been able to parse json string.");

  let success: bool = write_shortcuts_vdf(&shortcuts_vdf_path, shortcuts_data);

  if success {
    logger::log_to_file(app_handle.to_owned(), "Changes to shortcuts saved.", 0);
    return true;
  } else {
    logger::log_to_file(app_handle.to_owned(), "Changes to shortcuts failed.", 0);
    return false;
  }
}

#[tauri::command]
/// Downloads a file from a url.
async fn download_grid(app_handle: AppHandle, grid_url: String, dest_path: String) -> bool {
  logger::log_to_file(app_handle.to_owned(), format!("Downloading grid from {} to {}", grid_url, dest_path).as_str(), 0);

  let mut dest_file: File = File::create(&dest_path).expect("Dest path should have existed.");
  let response = reqwest::get(grid_url.clone()).await.expect("Should have been able to await request.");
  let response_bytes = response.bytes().await.expect("Should have been able to await getting response bytes.");

  let write_res = dest_file.write_all(&response_bytes);

  if write_res.is_ok() {
    logger::log_to_file(app_handle.to_owned(), format!("Download of {} finished.", grid_url.clone()).as_str(), 0);
    return true;
  } else {
    let err = write_res.err().expect("Request failed, error should have existed.");
    logger::log_to_file(app_handle.to_owned(), format!("Download of {} failed with {}.", grid_url.clone(), err.to_string()).as_str(), 0);
    return false;
  }
}

/// Adds the user's steam directory to Tauri FS and Asset scope.
fn add_steam_to_scope(app_handle: &AppHandle) {
  let steam_path = get_steam_root_dir();

  let fs_scope = app_handle.fs_scope();
  let asset_scope = app_handle.asset_protocol_scope();

  let fs_res = FsScope::allow_directory(&fs_scope, &steam_path, true);
  let asset_res = FsScope::allow_directory(&asset_scope, &steam_path, true);

  if fs_res.is_ok() && asset_res.is_ok() {
    logger::log_to_file(app_handle.to_owned(), "Added Steam directory to scope.", 0);
  } else if fs_res.is_err() {
    let err = fs_res.err().unwrap();
    logger::log_to_file(app_handle.to_owned(), format!("Error adding Steam directory to scope. FS Scope Error: {}", err.to_string()).as_str(), 0);
  } else if asset_res.is_err() {
    let err = asset_res.err().unwrap();
    logger::log_to_file(app_handle.to_owned(), format!("Error adding Steam directory to scope. Asset Scope Error: {}", err.to_string()).as_str(), 0);
  } else {
    let fs_err = fs_res.err().unwrap();
    let asset_err = asset_res.err().unwrap();
    logger::log_to_file(app_handle.to_owned(), format!("Error adding Steam directory to scope. FS Scope Error: {}. Asset Scope Error: {}", fs_err.to_string(), asset_err.to_string()).as_str(), 0);
  }
}


/// This app's main function.
fn main() {
  tauri::Builder::default()
    .invoke_handler(tauri::generate_handler![
      logger::clean_out_log,
      logger::log_to_file,
      steam::get_steam_users,
      steam::get_grids_directory,
      steam::get_library_cache_directory,
      steam::get_appinfo_path,
      steam::get_shortcuts_path,
      steam::get_localconfig_path,
      export_grids_to_zip,
      import_grids_from_zip,
      read_appinfo_vdf,
      read_shortcuts_vdf,
      read_localconfig_vdf,
      save_changes,
      write_shortcuts,
      download_grid
    ])
    .plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
      println!("{}, {argv:?}, {cwd}", app.package_info().name);

      app.emit_all("single-instance", Payload { args: argv, cwd }).unwrap();
    }))
    .setup(| app | {
      let app_handle = app.handle();
      logger::clean_out_log(app_handle.clone());
      add_steam_to_scope(&app_handle);
      Ok(())
    })
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}