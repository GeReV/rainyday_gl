extern crate gl;
extern crate sdl2;
#[macro_use]
extern crate failure;
#[macro_use]
extern crate render_gl_derive;
extern crate nalgebra;
extern crate ncollide2d;
extern crate rand;

mod background;
mod debug;
mod drop_quad;
mod droplet;
mod quad;
pub mod render_gl;
pub mod resources;
mod vertex;

use crate::debug::failure_to_string;
use crate::drop_quad::DropQuad;
use crate::droplet::Droplet;
use crate::quad::Quad;
use crate::render_gl::{Program, Texture};
use crate::vertex::Vertex;
use failure::err_msg;
use nalgebra as na;
use nalgebra::{Isometry2, Point2};
use ncollide2d as nc;
use ncollide2d::bounding_volume::BoundingSphere;
use ncollide2d::broad_phase::{BroadPhase, DBVTBroadPhase};
use ncollide2d::pipeline::{
    default_narrow_phase, CollisionGroups, CollisionObject, CollisionObjectSlabHandle,
    CollisionWorld, ContactEvent, GeometricQueryType,
};
use ncollide2d::shape::{Ball, ShapeHandle};
use rand::Rng;
use render_gl::buffer::*;
use resources::Resources;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

const DROPLET_COUNT: u32 = 5_000;

fn main() {
    if let Err(e) = run() {
        println!("{}", failure_to_string(e));
    }
}

fn run() -> Result<(), failure::Error> {
    let res = Resources::from_relative_exe_path(Path::new("assets")).unwrap();

    let sdl = sdl2::init().map_err(err_msg)?;

    let video_subsystem = sdl.video().map_err(err_msg)?;

    let gl_attr = video_subsystem.gl_attr();

    gl_attr.set_context_profile(sdl2::video::GLProfile::Core);
    gl_attr.set_context_version(4, 5);

    let initial_window_size: (i32, i32) = (1920, 1080);

    let window = video_subsystem
        .window(
            "Rain",
            initial_window_size.0 as u32,
            initial_window_size.1 as u32,
        )
        //        .fullscreen_desktop()
        .opengl()
        .resizable()
        .build()?;

    let _gl_context = window.gl_create_context().map_err(err_msg)?;

    let gl = gl::Gl::load_with(|s| {
        video_subsystem.gl_get_proc_address(s) as *const std::os::raw::c_void
    });

    let mut viewport =
        render_gl::Viewport::for_window(initial_window_size.0, initial_window_size.1);

    let distance = 10.0;

    let view: na::Matrix4<f32> = (na::Translation3::<f32>::from(na::Point3::origin().coords)
        * na::Translation3::<f32>::from(na::Vector3::z() * distance))
    .inverse()
    .to_homogeneous();

    let mut projection = na::Orthographic3::new(
        0.0,
        initial_window_size.0 as f32,
        0.0,
        initial_window_size.1 as f32,
        0.01,
        1000.0,
    );

    let matrix = projection.into_inner() * view;

    let texture = render_gl::Texture::from_res_rgb("textures/background.jpg")
        .with_gen_mipmaps()
        .load(&gl, &res)?;

    let texture_rc = Rc::<render_gl::Texture>::new(texture);

    let texture_buffer = render_gl::Texture::new(
        &gl,
        initial_window_size.0 as u32,
        initial_window_size.1 as u32,
    )?;

    let frame_buffer = render_gl::FrameBuffer::new(&gl);

    frame_buffer.bind();
    frame_buffer.attach_texture(&texture_buffer);

    let color_buffer2 = render_gl::ColorBuffer::from_color(na::Vector3::new(0.5, 0.6, 0.8));

    color_buffer2.set_used(&gl);
    color_buffer2.clear(&gl);

    frame_buffer.unbind();

    let background = background::Background::new(
        &res,
        &gl,
        texture_rc.clone(),
        initial_window_size.0 as u32,
        initial_window_size.1 as u32,
    )?;

    let quad = Quad::new(&gl)?;

    viewport.set_used(&gl);

    let color_buffer = render_gl::ColorBuffer::from_color(na::Vector3::new(0.3, 0.3, 0.5));

    color_buffer.set_used(&gl);

    let mut event_pump = sdl.event_pump().map_err(err_msg)?;

    let mut rng = rand::thread_rng();

    let mut droplets: Vec<Droplet> = (0..DROPLET_COUNT)
        .map(|_| {
            let x = rng.gen_range(0.0, viewport.w as f32);
            let y = rng.gen_range(0.0, viewport.h as f32);

            let size = rng.gen_range(1.5, 7.0);

            Droplet {
                x,
                y,
                size,
                collided: false,
                seed: 0,
                skipping: false,
                slowing: false,
                deleted: false,
                x_speed: 0.0,
                y_speed: 0.0,
            }
        })
        .collect();

    let mut world = CollisionWorld::new(0.2);

    let collision_group = CollisionGroups::new();
    let contacts_query = GeometricQueryType::Contacts(0.0, 0.0);

    let mut collision_handles = Vec::with_capacity(droplets.len());

    for (i, d) in droplets.iter().enumerate() {
        let shape_handle = ShapeHandle::new(Ball::new(d.size * 0.5));

        let handle = world
            .add(
                Isometry2::new(na::Vector2::new(d.x, d.y), na::zero()),
                shape_handle,
                collision_group,
                contacts_query,
                i,
            )
            .0;

        collision_handles.push(handle);
    }

    unsafe {
        gl.Enable(gl::BLEND);
        gl.BlendFunc(gl::SRC_ALPHA, gl::ONE_MINUS_SRC_ALPHA);
    }

    let mut instant = Instant::now();

    let program = render_gl::Program::from_res(&gl, &res, "shaders/drop")?;

    let mut updates = Vec::<(CollisionObjectSlabHandle, CollisionObjectSlabHandle)>::new();

    'main: loop {
        let now = Instant::now();
        let delta = now.duration_since(instant);
        instant = now;

        for event in event_pump.poll_iter() {
            match event {
                sdl2::event::Event::Quit { .. } => break 'main,
                sdl2::event::Event::KeyUp { .. } => break 'main,
                sdl2::event::Event::Window {
                    win_event: sdl2::event::WindowEvent::Resized(w, h),
                    ..
                } => {
                    viewport.update_size(w, h);
                    viewport.set_used(&gl);

                    projection.set_left_and_right(0.0, w as f32);
                    projection.set_bottom_and_top(0.0, h as f32);
                }
                _ => {}
            }
        }

        let resolution: na::Vector2<f32> =
            na::Vector2::<f32>::new(viewport.w as f32, viewport.h as f32);

        gravity_non_linear(&mut droplets, &mut world, &collision_handles, &delta);

        updates.clear();

        for ev in world.contact_events().iter().collect::<Vec<_>>() {
            if let ContactEvent::Started(handle1, handle2) = ev {
                if let (Some(obj1), Some(obj2)) = (
                    world.collision_object(*handle1),
                    world.collision_object(*handle2),
                ) {
                    let sphere1 = obj1.shape().local_bounding_sphere();
                    let sphere2 = obj2.shape().local_bounding_sphere();

                    let rad1 = sphere1.radius();
                    let rad2 = sphere2.radius();

                    let pair = if rad1 > rad2 {
                        (*handle1, *handle2)
                    } else if rad1 < rad2 {
                        (*handle2, *handle1)
                    } else if sphere1.center().y > sphere2.center().y {
                        (*handle1, *handle2)
                    } else {
                        (*handle2, *handle1)
                    };

                    updates.push(pair);
                }
            }
        }

        for (keep_handle, delete_handle) in updates.iter() {
            if let (Some(keep), Some(delete)) =
                world.collision_object_pair_mut(*keep_handle, *delete_handle)
            {
                let keep_droplet_index = *keep.data();
                let delete_droplet_index = *delete.data();

                let delete_droplet_size = droplets[delete_droplet_index].size;

                let keep_droplet = &mut droplets[keep_droplet_index];

                // TODO: How much does a droplet grow when is absorbs another?
                keep_droplet.size += delete_droplet_size.cbrt() * 0.5;

                keep.set_shape(ShapeHandle::new(Ball::new(keep_droplet.size)));
            }
        }

        for (_, delete_handle) in updates.iter() {
            if let Some(delete) = world.collision_object(*delete_handle) {
                droplets[*delete.data()].deleted = true;
                world.remove(&[*delete_handle]);
            }
        }

        color_buffer.clear(&gl);

        background.render(&gl, 1.0, &view, &matrix, &resolution);

        render_droplets(
            &gl,
            &program,
            &matrix,
            &resolution,
            texture_rc.clone(),
            &quad,
            &droplets,
        );

        window.gl_swap_window();
    }

    Ok(())
}

fn render_droplets(
    gl: &gl::Gl,
    program: &Program,
    matrix: &na::Matrix4<f32>,
    resolution: &na::Vector2<f32>,
    texture: Rc<Texture>,
    quad: &Quad,
    droplets: &Vec<Droplet>,
) {
    let program_matrix_location = program.get_uniform_location("MVP");
    let texture_location = program.get_uniform_location("Texture");
    let resolution_location = program.get_uniform_location("Resolution");

    program.set_used();

    if let Some(loc) = resolution_location {
        program.set_uniform_2f(loc, &resolution);
    }

    if let Some(loc) = program_matrix_location {
        program.set_uniform_matrix_4fv(loc, &matrix);
    }

    if let Some(loc) = texture_location {
        texture.bind_at(0);
        program.set_uniform_1i(loc, 0);
    }

    quad.vao.bind();

    let instance_vbo: ArrayBuffer = ArrayBuffer::new(&gl);
    instance_vbo.bind();

    let offsets: Vec<na::Vector3<f32>> = droplets
        .iter()
        .filter(|&d| !d.deleted)
        .map(|d| na::Vector3::new(d.x, d.y, d.size))
        .collect();

    instance_vbo.static_draw_data(&offsets);

    instance_vbo.unbind();

    instance_vbo.bind();
    unsafe {
        gl.EnableVertexAttribArray(3);
        gl.VertexAttribPointer(
            3,
            3,         // the number of components per generic vertex attribute
            gl::FLOAT, // data type
            gl::FALSE,
            std::mem::size_of::<na::Vector3<f32>>() as gl::types::GLint,
            0 as *const gl::types::GLvoid,
        );
    }
    instance_vbo.unbind();

    unsafe {
        gl.VertexAttribDivisor(3, 1);
    }

    unsafe {
        gl.DrawElementsInstanced(
            gl::TRIANGLES,
            6,
            gl::UNSIGNED_BYTE,
            ::std::ptr::null(),
            offsets.len() as i32,
        );
    }
    quad.vao.unbind();
}

const PRIVATE_GRAVITY_FORCE_FACTOR_Y: f32 = 0.2;
const PRIVATE_GRAVITY_FORCE_FACTOR_X: f32 = 0.0;

fn gravity_non_linear(
    droplets: &mut Vec<Droplet>,
    world: &mut CollisionWorld<f32, usize>,
    collision_handles: &Vec<CollisionObjectSlabHandle>,
    dt: &Duration,
) {
    let mut rng = rand::thread_rng();

    let gravity_y = PRIVATE_GRAVITY_FORCE_FACTOR_Y * dt.as_secs_f32();

    for i in 0..droplets.len() {
        let droplet = &mut droplets[i];

        if droplet.deleted {
            continue;
        }

        if droplet.collided {
            droplet.collided = false;
            droplet.seed = (droplet.size * rng.gen::<f32>() * 100.0).floor() as i32;
            droplet.skipping = false;
            droplet.slowing = false;
        } else if droplet.seed <= 0 {
            droplet.seed = (droplet.size * rng.gen::<f32>() * 100.0).floor() as i32;
            droplet.skipping = droplet.skipping == false;
            droplet.slowing = true;
        }

        droplet.seed -= 1;

        if droplet.y_speed > 0.0 {
            if droplet.slowing {
                droplet.y_speed *= 0.9;
                droplet.x_speed *= 0.9;
                if droplet.y_speed < gravity_y {
                    droplet.slowing = false;
                }
            } else if droplet.skipping {
                droplet.y_speed = gravity_y;
                droplet.x_speed = PRIVATE_GRAVITY_FORCE_FACTOR_X;
            } else {
                droplet.y_speed += gravity_y * droplet.size;
                droplet.x_speed += PRIVATE_GRAVITY_FORCE_FACTOR_X * droplet.size;
            }
        } else {
            droplet.y_speed = gravity_y;
            droplet.x_speed = PRIVATE_GRAVITY_FORCE_FACTOR_X;
        }

        //        if this.options.gravityAngleVariance != 0 {
        //            droplet.x_speed +=
        //                (rnd.gen() * 2 - 1) * droplet.y_speed * this.options.gravityAngleVariance
        //        }

        droplet.y -= droplet.y_speed;
        droplet.x += droplet.x_speed;

        if droplet.x_speed != 0.0 || droplet.y_speed != 0.0 {
            let handle = collision_handles[i];

            let object = world.get_mut(handle).unwrap();

            object.set_position(Isometry2::new(
                na::Vector2::new(droplet.x, droplet.y),
                na::zero(),
            ));
        }
    }

    world.update();
}
