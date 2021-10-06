use crate::position_parser::{SimulationData, TimePoint};
use crate::util;

use glam::Vec3A;
use once_cell::sync::OnceCell;
use plotters::prelude::*;
use rand::{distributions::Alphanumeric, Rng};

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

struct Parameter {
    name: String,
    optim: tpe::TpeOptimizer,
}

type State = Arc<Mutex<StateImpl>>;

struct StateImpl {
    params: Vec<Parameter>,

    //Mapping of parameter values to the fitness score
    results: Vec<(HashMap<String, f64>, f64)>,
}

static RUNNING: AtomicBool = AtomicBool::new(true);
static PATH: OnceCell<String> = OnceCell::new();
static STATE: OnceCell<State> = OnceCell::new();
static BASE_ARGUMENTS: [&str; 1] = ["--duration=360"];
static BEST_FITNESS: atomic_float::AtomicF64 = atomic_float::AtomicF64::new(1000.0);

pub fn run(path: &str) {
    ctrlc::set_handler(|| {
        RUNNING.store(false, Ordering::Relaxed);
        println!(" Shutting down runners");
    })
    .expect("failed to to set Control-C handler");

    let param_max = 10.0;
    let _ = STATE.set(Arc::new(Mutex::new(StateImpl {
        params: vec![
            Parameter {
                name: "a".to_owned(),
                optim: tpe::TpeOptimizer::new(
                    tpe::parzen_estimator(),
                    tpe::range(0.0, param_max).unwrap(),
                ),
            },
            Parameter {
                name: "r".to_owned(),
                optim: tpe::TpeOptimizer::new(
                    tpe::parzen_estimator(),
                    tpe::range(0.0, param_max).unwrap(),
                ),
            },
        ],
        results: Vec::new(),
    })));
    for param in STATE.get().unwrap().lock().unwrap().params.iter_mut() {
        // Fill in default values so parameters start around 1 by default
        param.optim.tell(1.0, 1000.0).unwrap();
    }

    let mut threads = Vec::new();
    let _ = PATH.set(path.to_owned());
    for _ in 0..num_cpus::get() {
        threads.push(std::thread::spawn(run_thread));
    }
    println!("Runners started");
    for thread in threads {
        let _ = thread.join();
    }

    println!("All runners stopped");
    let state = STATE.get().unwrap().lock().unwrap();
    println!("Exporting results from {} simulations", state.results.len());

    let width = 50;
    let height = 40;

    //Map parameter values to integer coordinates so we can draw them as pixels
    let params_to_draw: Vec<&String> = state.results[0].0.keys().take(2).collect();
    let mut pixel_map = HashMap::new();
    for result in &state.results {
        let params_used = &result.0;
        let x = params_used[params_to_draw[0]];
        let y = params_used[params_to_draw[1]];
        let px: usize = util::map(0.0, param_max, x, 0.0, width as f64) as usize;
        let py: usize = util::map(0.0, param_max, y, 0.0, height as f64) as usize;
        let fitness = result.1;

        let key = (px, py);
        pixel_map
            .entry(key)
            .or_insert_with(|| (Vec::new(), x, y))
            .0
            .push(fitness);
    }

    //Find min and max fitness scores for color interpolation
    let mut fitness_scores: Vec<f64> = state
        .results
        .iter()
        .map(|(_, s)| *s)
        .filter(|v| !v.is_nan())
        .collect();

    fitness_scores.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let min = *fitness_scores.first().unwrap();
    let max = *fitness_scores.last().unwrap();
    println!("min {} max {}", min, max);

    let out_file_name: &'static str = "hot_cold.png";
    let root = BitMapBackend::new(out_file_name, (width, height)).into_drawing_area();

    root.fill(&WHITE).unwrap();

    let mut chart = ChartBuilder::on(&root)
        .x_label_area_size(0)
        .y_label_area_size(0)
        .build_cartesian_2d(0.0..param_max, 0.0..param_max)
        .unwrap();

    chart
        .configure_mesh()
        .disable_x_mesh()
        .disable_y_mesh()
        .draw()
        .unwrap();

    let plotting_area = chart.plotting_area();

    for (_, data) in pixel_map {
        let fitnesses = &data.0;
        let x = data.1;
        let y = data.2;

        let average_fitness = fitnesses.iter().sum::<f64>() / fitnesses.len() as f64;
        let good = util::map(min, max, average_fitness, 1.0, 0.0);
        let r = util::map(0.0, 1.0, good, 255.0, 0.0) as u8;
        let g = util::map(0.0, 1.0, good, 0.0, 255.0) as u8;
        let b = util::map(0.0, 1.0, good, 100.0, 200.0) as u8;
        plotting_area
            .draw_pixel((x, y), &RGBColor(r, g, b))
            .unwrap();
    }

    /*for (x, y, c) in mandelbrot_set(xr, yr, (pw as usize, ph as usize), 100) {
        if c != 100 {
            plotting_area
                .draw_pixel((x, y), &HSLColor(c as f64 / 100.0, 1.0, 0.5))
                .unwrap();
        } else {
            plotting_area.draw_pixel((x, y), &BLACK).unwrap();
        }
    }*/

    // To avoid the IO failure being ignored silently, we manually call the present function
    root.present().expect("Unable to write result to file, please make sure 'plotters-doc-data' dir exists under current dir");
    println!("Result has been saved to {}", out_file_name);
}

fn run_binary(
    rel_working_dir: &str,
    rel_bin_path: &str,
    args: &Vec<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut base = std::env::current_dir().unwrap();
    base.push(rel_working_dir);
    let lib_path = {
        let mut base = base.clone();
        base.push("build");
        base.push("lib");
        base
    };
    let current_dir = base.clone();
    base.push(rel_bin_path);
    let bin_path = base;

    if Command::new(bin_path)
        .current_dir(current_dir)
        .env("LD_LIBRARY_PATH", lib_path.to_str().unwrap())
        .args(args)
        .spawn()?
        .wait()?
        .success()
    {
        Ok(())
    } else {
        Ok(())
    }
}

fn run_thread() {
    let mut rng = rand::thread_rng();
    let mut param_map = HashMap::new();
    let mut args: Vec<String> = Vec::new();
    for arg in BASE_ARGUMENTS.iter() {
        args.push((*arg).to_owned());
    }

    while RUNNING.load(Ordering::Relaxed) {
        let pos_file_name: String = rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(10)
            .map(char::from)
            .collect();

        //Keep base arguments
        args.resize(BASE_ARGUMENTS.len(), String::new());

        let ns3_path = PATH.get().unwrap();
        let mut buf = PathBuf::from(ns3_path);
        buf.push(pos_file_name);
        buf.set_extension("csv");
        let mut positions_file = std::env::current_dir().unwrap();
        positions_file.push(buf);
        args.push(format!(
            "--positionsFile={}",
            &positions_file.to_str().unwrap()
        ));

        {
            let mut state = STATE.get().unwrap().lock().unwrap();
            param_map.clear();
            for param in state.params.iter_mut() {
                let value = param.optim.ask(&mut rng).unwrap();
                param_map.insert(param.name.clone(), value);
                args.push(format!("--{}={}", param.name, value));
            }
        };

        //Run simulation
        match run_binary(&ns3_path, "build/scratch/non-ideal/non-ideal", &args) {
            Ok(_) => match run_analysis(&positions_file, &param_map, &positions_file) {
                Ok(_) => {}
                Err(err) => {
                    println!("Error while doing analysis: {}", err);
                }
            },
            Err(err) => {
                println!("Error while running waf: {}", err);
                let _ = std::fs::remove_file(positions_file);
            }
        }
    }
    println!("Runner exiting cleanly");
}

fn get_fitness(data: &mut SimulationData) -> f64 {
    let time_step = 0.1;
    let mut time = 0.0;
    let mut last_poses = HashMap::new();
    let uavs = data.uavs.clone();
    let central_node = uavs.iter().min().unwrap();

    let mut all_distances = Vec::new();
    let mut all_velocities = Vec::new();
    let mut under_mad_threshold_time = None;
    while time <= data.simulation_length {
        let mut distances: Vec<f64> = Vec::new();
        let mut velocities: Vec<f64> = Vec::new();

        let central_pos = data.pos_at_time(TimePoint(time), *central_node).unwrap();
        for uav in &uavs {
            if let Some(now_pos) = data.pos_at_time(TimePoint(time), *uav) {
                match last_poses.get(uav) {
                    None => {}
                    Some((last_pos, last_time)) => {
                        let pos_delta = now_pos - *last_pos;
                        let time_delta = time - last_time;
                        let velocity: Vec3A = pos_delta / time_delta;
                        velocities.push(velocity.length() as f64);
                    }
                }
                last_poses.insert(uav, (now_pos, time));
                if uav != central_node {
                    distances.push((now_pos - central_pos).length() as f64);
                }
            }
        }
        let distances_mean = rgsl::statistics::mean(&distances, 1, distances.len());
        let mad_of_distance = rgsl::statistics::absdev(&distances, 1, distances.len());
        let mean_velocity = rgsl::statistics::mean(&velocities, 1, velocities.len());
        let mad_percent = mad_of_distance * distances_mean * 100.0;
        let mad_threshold = 30.0;
        match under_mad_threshold_time.clone() {
            Some(_) => {
                if mad_percent >= mad_threshold {
                    //Too high to hold streak
                    under_mad_threshold_time = None;
                }
            }
            None => {
                if mad_percent < mad_threshold {
                    //Start streak
                    under_mad_threshold_time = Some(time);
                }
            }
        }
        all_distances.push((time, distances_mean));
        all_velocities.push((time, mean_velocity));
        //println!("T: {}, V: {}, D: {}", time, mean_velocity, mad_of_distance);

        time += time_step;
    }
    let stable_time = under_mad_threshold_time.unwrap_or(360.0) as f64;
    let mean_velocity: f64 =
        all_velocities.iter().map(|(_, v)| *v).sum::<f64>() / all_velocities.len() as f64;

    let average_distance =
        all_distances.iter().map(|(_, v)| *v).sum::<f64>() / all_distances.len() as f64;

    let desired_distance_cost = 200.0 * (3.0 - average_distance).abs();
    let stable_time_cost = 1.0 * stable_time;
    let velocity_cost = 250.0 * mean_velocity;
    println!(
        "Final costs: distance: {}, stable time: {}, vel: {}",
        desired_distance_cost, stable_time_cost, velocity_cost
    );

    desired_distance_cost + stable_time_cost + velocity_cost
}

fn run_analysis(
    pos_path: &PathBuf,
    param_map: &HashMap<String, f64>,
    positions_file: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    //let start = Instant::now();
    let positions = String::from_utf8(std::fs::read(&pos_path)?)?;
    let mut data = SimulationData::parse(&positions)?;
    let fitness = get_fitness(&mut data);
    println!("FITNESS: {}", fitness);
    {
        let mut state = STATE.get().unwrap().lock().unwrap();
        for param in state.params.iter_mut() {
            let value = param_map.get(&param.name).unwrap();
            param.optim.tell(*value, fitness).unwrap();
        }
        state.results.push((param_map.clone(), fitness));
    }
    let old_fitness = BEST_FITNESS.load(Ordering::Relaxed);
    if fitness < old_fitness {
        //If multiple threads get in here we don't really care...
        BEST_FITNESS.store(fitness, Ordering::Relaxed);
        let src = positions_file.clone();
        let mut dest = positions_file.clone();
        dest.pop(); //Pop positions csv file name
        dest.push("out");
        dest.push(format!("{}.csv", fitness));
        std::fs::copy(src, dest).unwrap();
        println!("Got best fitness: {} for params: {:?}", fitness, param_map);
    }

    if let Some(err) = std::fs::remove_file(pos_path).err() {
        println!(
            "failed to delete temp positions file: {} - {}",
            pos_path.to_str().unwrap(),
            err
        );
    }
    /*println!(
        "Parsing took: {} ms",
        (Instant::now() - start).as_micros() as f64 / 1000.0f64
    );*/
    Ok(())
}
