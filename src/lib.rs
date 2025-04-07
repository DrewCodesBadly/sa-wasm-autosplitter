use std::{cmp::min, collections::HashMap};

use asr::{deep_pointer::DeepPointer, future::next_tick, print_message, settings::Gui, signature::Signature, string::{ArrayCString, ArrayWString}, timer::{pause_game_time, reset, resume_game_time, set_variable, split, start}, watcher::Watcher, Address, Process};

asr::async_main!(stable);

// Adapted from the Solar Ash ASL autosplitter.

// Used signatures, declared static as the crate says to.
static FNAME_POOL_SIG: Signature<11> = Signature::new("74 09 48 8D 15 ?? ?? ?? ?? EB 16");
static UWORLD_SIG: Signature<16> = Signature::new("0F 2E ?? 74 ?? 48 8B 1D ?? ?? ?? ?? 48 85 DB 74");

// List of boss kill flag strings
static BOSS_KILL_FLAGS: [&str; 6] = [
    "Vale_Starseed_Remnant",
    "Woods_OldCity_Remnant",
    "Woods_IronRootBasin_Remnant",
    "Shroom_GhostCoppice_Remnant",
    "Beach_AcidLagoon_SwordRemnant",
    "Shroom_Overflow_Remnant",
];

static EYE_SAVE_FLAGS: [&str; 26] = [
    "Vale_Starseed_StaticRemnantB",
    "Vale_Starseed_StaticRemnantC",
    "Vale_StaticRemnantD",
    "Woods_Cliffside_StaticRemnantA",
    "Woods_ClockTower_StaticRemnantA",
    "Woods_OldCity_StaticRemnantA",
    "Woods_OldCity_StaticRemnantB",
    "Woods_ForestAltar_StaticRemnantA",
    "Woods_IronRootHighlands_StaticRemnantA",
    "Woods_IronRootHighlands_StaticRemnantB",
    "Woods_IronRootBasin_StaticRemnantA",
    "Shroom_MagmaOutlets_StaticRemnantB",
    "Shroom_GhostCoppice_StaticRemnantA",
    "Shroom_Cathedral_StaticRemnantA",
    "Shroom_Archives_StaticRemnantA",
    "Shroom_MagmaOutlets_StaticRemnantA",
    "Beach_AcidLagoon_StaticRemnantA",
    "Beach_Pavilion_StaticRemnantA",
    "Beach_Frigate_StaticRemnantA",
    "Beach_PalaceGrounds_StaticRemnantA",
    "Beach_PalaceUnderGround_StaticRemnantA",
    "Shroom_Overflow_StaticRemnantA",
    "Shroom_FungusTowers_StaticRemnantA",
    "Shroom_ShatteredPeak_StaticRemnantA",
    "Shroom_Graveyard_MinorRemnantA",
    "Shroom_Overflow_StaticRemnantB",
];

#[derive(Gui)]
struct Settings {
    #[default = true]
    split_on_boss_kills: bool,
    split_on_bad_ending: bool,
    split_on_eye_complete: bool,
}

async fn main() {
    let mut settings = Settings::register();
    loop {
        let process = Process::wait_attach("Solar-Win64-Shipping").await;
        process
            .until_closes(async {
                // Corresponds to the init function in ASL
                // Loops until needed memory addresses are successfully found from signatures.
                // Should only need to be done once but repeats in case it fails.
                let mut fname_pool: Address;
                let mut uworld: Address;
                loop {
                    let (main_module_address, main_module_size) = process.get_module_range("Solar-Win64-Shipping.exe").unwrap();

                    #[cfg(debug_assertions)]
                    print_message(&format!("Module size: {:?}", main_module_size));

                    // my rust is simply beyond your comprehension (this is the stupidest code I've
                    // ever written)
                    fname_pool = FNAME_POOL_SIG.scan_process_range(&process, (main_module_address, main_module_size))
                        .map(|a| process.read::<i32>(a + 0x5).ok()
                            .map(|offset| a + offset + 0x9)
                            .unwrap_or(Address::NULL))
                        .unwrap_or(Address::NULL);
                    uworld = UWORLD_SIG.scan_process_range(&process, (main_module_address, main_module_size))
                        .map(|a| process.read::<i32>(a + 0x8).ok()
                            .map(|offset| a + offset + 0xC)
                            .unwrap_or(Address::NULL))
                        .unwrap_or(Address::NULL);
    
                    if fname_pool != Address::NULL && uworld != Address::NULL {
                        break;
                    }
                    // Debug
                    if fname_pool == Address::NULL {
                        if uworld == Address::NULL {
                            print_message("Failed to get both addresses.");
                        } else {
                            print_message("Failed to get fname_pool");
                        }
                    } else {
                        print_message("Failed to get uworld");
                    }
                }

                // Create pointers to useful values from the given info
                // For some reason ASR pointer paths have a base_address field which should just be
                // set to 0 or it will add that and the first offset.
                let game_state_ptr = DeepPointer::<3>::new_64bit(Address::new(0x0), &[uworld.value(), 0x128, 0x5E0]);
                let save_flag_count_ptr = DeepPointer::<3>::new_64bit(Address::new(0x0), &[uworld.value(), 0x188, 0x208]);
                let save_flag_ptr_ptr = DeepPointer::<3>::new_64bit(Address::new(0x0), &[uworld.value(), 0x188, 0x200]);
                let current_map_ptr = DeepPointer::<3>::new_64bit(Address::new(0x0), &[uworld.value(), 0x428, 0x0]);
                // Watchers
                let mut game_state = Watcher::<u8>::new();
                let mut save_flag_count = Watcher::<i32>::new();
                let mut current_map = Watcher::<ArrayWString::<35>>::new();
                let mut newest_save_flag = Watcher::<String>::new();
                let mut fname_dict = HashMap::<i64, String>::new();

                // Variables
                let mut start_on_gain_control = false;
                let mut split_on_lose_control = false;
                // This is used to try to fix the issues with an extra autosplit after final cyd
                // They may only have happened on the old ASL autosplitter but just in case
                let mut boss_splits_triggered = 0;

                #[cfg(debug_assertions)]
                print_message("Successfully entered main loop");

                loop {
                    settings.update();

                    // Update watchers
                    // TODO: Maybe do something with results? Seem useless though.
                    let _ = game_state.update(game_state_ptr.deref::<u8>(&process).ok());
                    let _ = save_flag_count.update(save_flag_count_ptr.deref::<i32>(&process).ok());
                    let _ = current_map.update(current_map_ptr.deref::<ArrayWString::<35>>(&process).ok());
                    if let Some(p) = save_flag_count.pair {
                        let updated_newest_save_flag = get_name_from_fname(&mut fname_dict, &process, save_flag_ptr_ptr, fname_pool, p.current);

                        #[cfg(debug_assertions)]
                        if let Some(s) = &updated_newest_save_flag {
                            set_variable("newest_save_flag", &s);
                        }

                        let _ = newest_save_flag.update(updated_newest_save_flag);
                        
                        #[cfg(debug_assertions)]
                        set_variable("save_flag_count", &p.current.to_string());
                    }

                    // Everything reliant on current_map -
                    // this sets a bool for splitting, starts the timer, and resets.
                    // The watcher system and working around rust's Optional here is a bit strange
                    let mut at_title = false;
                    if let Some(p) = current_map.pair {
                        if p.current.matches_str("/Game/Maps/Cutscenes/Opening_Master") {
                            start_on_gain_control = true;
                            if p.old != p.current {
                                reset();
                            }
                        }
                        else if p.current.matches_str("/Game/Maps/TitleNMainMenu") {
                            at_title = true;
                        }

                        #[cfg(debug_assertions)]
                        set_variable("current map", &String::from_utf16(p.current.as_slice()).unwrap_or_default());
                    }
                    if start_on_gain_control {
                        if let Some(p) = game_state.pair {
                            if p.current == 4 && p.old == 3 {
                                boss_splits_triggered = 0;
                                start_on_gain_control = false;
                                split_on_lose_control = false;
                                start();
                            }
                        }
                    }

                    // Load removal
                    let playing: bool;
                    if let Some(p) = game_state.pair {
                        if p.current == 3 || p.current == 4 {
                            playing = true;
                        } else {
                            playing = false;
                        }

                        #[cfg(debug_assertions)]
                        set_variable("game_state", &p.current.to_string());
                    } else {
                        playing = false;
                    }
                    if !playing || at_title {
                        pause_game_time();
                    } else {
                        resume_game_time();
                    }

                    // Splitting
                    if settings.split_on_boss_kills {
                        if let Some(p) = &newest_save_flag.pair {
                            if p.old != p.current {
                                for flag in BOSS_KILL_FLAGS {
                                    if p.current == flag {
                                        split();
                                        boss_splits_triggered += 1;
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    if settings.split_on_eye_complete && boss_splits_triggered < 6 {
                        if let Some(p) = &newest_save_flag.pair {
                            if p.old != p.current {
                                for flag in EYE_SAVE_FLAGS {
                                    if p.current == flag {
                                        split();
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    if settings.split_on_bad_ending {
                        if let Some(p) = &newest_save_flag.pair {
                            if p.old != p.current && p.old.contains("DISABLE_SAVING") {
                                if let Some(count_pair) = save_flag_count.pair {
                                    if count_pair.current == 2 {
                                        split_on_lose_control = true;
                                    }
                                }
                            }
                        }
                        if split_on_lose_control {
                            if let Some(p) = game_state.pair {
                                if p.current == 3 && p.old == 4 {
                                    split_on_lose_control = false;
                                    split();
                                }
                            }

                            // Checks to reset this flag if the run isn't finished.
                            if let Some(p) = current_map.pair {
                                if p.current.matches_str("/Game/Maps/TitleNMainMenu") {
                                    split_on_lose_control = false;
                                }
                            }
                            if let Some(p) = save_flag_count.pair {
                                if p.current > 10 {
                                    split_on_lose_control = false;
                                }
                            }
                        }
                    }

                    next_tick().await;
                }
            })
            .await;
    }
}

// Matches the similarly named function in the ASL autosplitter, mostly
fn get_name_from_fname(fname_dict: &mut HashMap<i64, String>, process: &Process, save_flag_ptr: DeepPointer::<3>, fname_pool: Address, save_flag_count: i32) -> Option<String> {
    if save_flag_count <= 0 { return None; } 
    let ptr = save_flag_ptr.deref::<u64>(process).ok()? + (0x8 * (save_flag_count - 1) as u64);
    let id: i64 = process.read(ptr).ok()?;

    #[cfg(debug_assertions)]
    set_variable("save_flag_id", &id.to_string());

    // Todo: Could be gotten from the i64 ID I'm just lazy and directly copying the asr code
    let key: i32 = process.read(ptr).ok()?;

    #[cfg(debug_assertions)]
    set_variable("save_flag_key", &key.to_string());
    // let partial: i32 = process.read(ptr + 0x4).ok()?;
    let chunk_offset = key >> 16;
    #[cfg(debug_assertions)]
    set_variable("save_flag_chunk_offset", &chunk_offset.to_string());
    let name_offset = key as u16; 
    #[cfg(debug_assertions)]
    set_variable("save_flag_name_offset", &name_offset.to_string());
    let name_pool_chunk: Address = Address::new(process.read(fname_pool + ((chunk_offset + 0x2) * 0x8)).ok()?);
    // Cast to u64 to avoid overflow crash
    let name_entry: i16 = process.read(name_pool_chunk + 0x2 * name_offset as u64).ok()?;
    let name_length = name_entry >> 6;
    let result: ArrayCString::<64>; 
    // Same reason for u64 cast here
    result = process.read(name_pool_chunk + 2 * name_offset as u64 + 2).ok()?;
    // let string_result = String::from_utf8(result.as_bytes()[0..name_length as usize].to_vec()).unwrap_or_default();
    // In case a really long name causes a crash, happened once and I think this should solve it
    // I won't make the name buffer longer than it needs to be since it seems silly that you would 
    // actually need to read the full name if it's that long
    let name_length = min(name_length, 64);
    let string_result = String::from_utf8(result.as_bytes()[0..name_length as usize].to_vec()).unwrap_or_default();
    fname_dict.insert(id, string_result.clone());
    Some(string_result)
}
