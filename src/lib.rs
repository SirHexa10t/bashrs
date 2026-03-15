pub mod commands;


// use libc::{c_char, c_int};
// use std::ffi::CStr;
// use std::ptr;
//
// #[repr(C)]
// pub struct WordList {
//     pub word: *mut libc::c_void,
//     pub next: *mut WordList,
// }
//
// #[repr(C)]
// pub struct Builtin {
//     pub name: *const c_char,
//     pub function: extern "C" fn(*mut WordList) -> c_int,
//     pub flags: c_int,
//     pub long_doc: *const *const c_char,
//     pub short_doc: *const c_char,
//     pub handle: *mut libc::c_void,
// }
//
// extern "C" fn hello_world(_list: *mut WordList) -> c_int {
//     println!("hello world from rust builtin");
//     0
// }
//
// #[unsafe(no_mangle)]
// pub static mut hello_world_struct: Builtin = Builtin {
//     name: b"hello_world\0".as_ptr() as *const c_char,
//     function: hello_world,
//     flags: 0,
//     long_doc: ptr::null(),
//     short_doc: b"Print hello world\0".as_ptr() as *const c_char,
//     handle: ptr::null_mut(),
// };

//
//
// #[cfg(test)]
// mod tests {
//     use super::*;
//
//     #[test]
//     fn it_works() {
//         let result = add(2, 2);
//         assert_eq!(result, 4);
//     }
// }
