use std::collections::HashMap;
use std::fs::read_to_string;
use std::io::{BufReader, Read, Write};
use std::net::UdpSocket;
use std::ops::Rem;
use std::path::Path;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;
use lazy_static::lazy_static;
use hachimi_plugin_sdk::{api::{Hachimi, HachimiApi}, hachimi_plugin, sys::InitResult};
use hachimi_plugin_sdk::il2cpp::types::{Il2CppObject, Il2CppArray, Il2CppString};
use log::info;
use log::error;
use hachimi_plugin_sdk::il2cpp::helpers::Array;

static mut API: Option<HachimiApi> = None;

static mut FROM_BASE_64_STRING_ORIG: usize = 0;
static mut CREATE_DECRYPTOR_ORIG: usize = 0;
static mut WRITE_ORIG: usize = 0;

static VERSION: &str = "0.2.0";


static CONFIG_DIRECTORY: &str = "CarrotBlender";
static CONFIG_FILE_NAME: &str = "config.properties";

//Completely arbitrary, might need to be smaller if we want to send packets over the network
static DEFAULT_MAX_PARTIAL_MESSAGE_SIZE: usize = 30000;
//Delay between each chunk in ms. Note: some responses are 20+ chunks long!
static DEFAULT_DELAY_BETWEEN_CHUNKS_MS: u64 = 50;
static DEFAULT_UL_HOST: &str = "127.0.0.1";
static DEFAULT_UL_PORT: &str = "17229";



lazy_static!{
    static ref socket: Mutex<UdpSocket> = Mutex::new(UdpSocket::bind("0.0.0.0:0").expect( "Failed to bind socket"));



    static ref CONFIG_MAP: HashMap<String, String> =
    {
        let config_file_path = [CONFIG_DIRECTORY, CONFIG_FILE_NAME].join("/");
        let file = std::fs::File::open(&config_file_path);
        if file.is_err()
        {
            error!( "Failed to open config file: {config_file_path} : {}", file.unwrap_err() );
            return HashMap::new();
        }
        let dst_map = java_properties::read(BufReader::new(file.unwrap()));
        if dst_map.is_err()
        {
            error!( "Failed to read config file: {config_file_path} : {}", dst_map.unwrap_err() );
            return HashMap::new();
        }
        return dst_map.unwrap()
    };

    static ref DELAY_BETWEEN_CHUNKS_MS: u64 = //delay after sending each chunk
    {
        let contains = CONFIG_MAP.get( "delay_between_chunks_ms" );
        if contains.is_none()
        {
            info!( "delay_between_chunks_ms not found in config file, using default of {DEFAULT_DELAY_BETWEEN_CHUNKS_MS}" );
            return DEFAULT_DELAY_BETWEEN_CHUNKS_MS;
        }
        let res = contains.unwrap().parse::<u64>();
        if res.is_err()
        {
            error!( "Failed to parse delay_between_chunks_ms: {}", res.unwrap_err() );
            return DEFAULT_DELAY_BETWEEN_CHUNKS_MS;
        }
        return res.unwrap();

    };

    static ref PARTIAL_MESSAGE_SIZE: usize =
    {
        let contains = CONFIG_MAP.get( "max_partial_message_size" );
        if contains.is_none()
        {
            info!( "max_partial_message_size not found in config file, using default of {DEFAULT_MAX_PARTIAL_MESSAGE_SIZE}" );
            return DEFAULT_MAX_PARTIAL_MESSAGE_SIZE;
        }
        let res = contains.unwrap().parse::<usize>();
        if res.is_err()
        {
            error!( "Failed to parse max_partial_message_size: {}", res.unwrap_err() );
            return DEFAULT_MAX_PARTIAL_MESSAGE_SIZE;
        }
        return res.unwrap();
    };

    static ref UL_HOST: String =
    {
        let contains = CONFIG_MAP.get( "host" );
        let temp;
        if contains.is_none()
        {
            info!( "host not found in config file, using default of {DEFAULT_UL_HOST}" );
            temp = DEFAULT_UL_HOST.to_string();
        }
        else
        {
            temp = contains.unwrap().to_string();
            info!( "host found config file: {temp}" );
        }
        return temp.trim().to_string();
    };

    static ref UL_PORT: String =
    {
        let contains = CONFIG_MAP.get( "port" );
        let temp;
        if contains.is_none()
        {
            info!( "port not found in config file, using default of {DEFAULT_UL_PORT}" );
            temp = DEFAULT_UL_PORT.to_string();
        }
        else
        {
            temp = contains.unwrap().to_string();
            info!( "port found config file: {temp}" );
        }
        return temp.trim().to_string();
    };

    static ref UL_ADDRESS: String =
    {
        [UL_HOST.clone(), UL_PORT.clone()].join(":")
    };


}


static DEFAULT_CONFIG_FILE_CONTENTS: &[u8] = b"\
    # CarrotBlender config file\n\
    # \n\
    # Don't edit this file unless you know what you're doing.\n\
    # Lines starting with # are comments\n\
    # If a config item is not set, it will use its default value.\n\
    \n\
    # Uma Launcher IP address/hostname and port. Should be set to 127.0.0.1 and 17229, respectively.\n\
    #host=127.0.0.1\n\
    #port=17229\n\
    #\n\
    # Maximum size of a response message. If a response is larger than this, it will be split into multiple chunks, each of which is no larger than this value.\n\
    # The default value of 30000 is appropriate in most cases, but a smaller value may be needed if the packet needs to go across the network.\n\
    #max_partial_message_size=30000\n\
    # The delay (in milliseconds) between each chunk being sent. You might need to increase this value if Uma Launcher can't keep up with the incoming packets.\n\
    #delay_between_chunks_ms=50\n\
    ";




#[hachimi_plugin]
pub fn main(api: HachimiApi) -> InitResult {
    unsafe { API = Some(api); }
    // Silently ignore log init errors
    _ = hachimi_plugin_sdk::log::init(api, log::Level::Info);


    info!("CarrotBlender {VERSION} loaded!");
    let config_file_path = [CONFIG_DIRECTORY, CONFIG_FILE_NAME].join("/");
    if !Path::new(config_file_path.as_str()).exists() {
        create_default_config_file(&config_file_path);
    }
    //Config file is read in lazy_static block

    let hachimi = Hachimi::instance(&api);
    let il2cpp = api.il2cpp();
    let interceptor = hachimi.interceptor();

    let image = il2cpp.get_assembly_image(c"mscorlib.dll");
    if image.is_null() {
        error!("Failed to get mscorlib.dll image!");
        return InitResult::Error;
    };


    let Convert = il2cpp.get_class(image, c"System", c"Convert");
    if Convert.is_null() {
        error!("Failed to get System.Convert class!");
        return InitResult::Error;
    };
    let FromBase64String_addr = il2cpp.get_method_addr(Convert, c"FromBase64String", 1);
    if FromBase64String_addr == 0 {
        error!("Failed to get FromBase64String address!");
        return InitResult::Error;
    }
    if let Some(trampoline) = interceptor.hook(FromBase64String_addr, FromBase64String as _) {
        unsafe { FROM_BASE_64_STRING_ORIG = trampoline; }
    }


    let RijndaelManaged = il2cpp.get_class(image, c"System.Security.Cryptography", c"RijndaelManaged");
    if RijndaelManaged.is_null() {
        error!("Failed to get System.Security.Cryptography.RijndaelManaged class!");
        return InitResult::Error;
    };
    let CreateDecryptor_addr =  il2cpp.get_method_addr(RijndaelManaged, c"CreateDecryptor", 2);
    if FromBase64String_addr == 0 {
        error!("Failed to get CreateDecryptor address!");
        return InitResult::Error;
    }
    if let Some(trampoline) = interceptor.hook(CreateDecryptor_addr, CreateDecryptor as _) {
        unsafe { CREATE_DECRYPTOR_ORIG = trampoline; }
    }


    let CryptoStream = il2cpp.get_class(image, c"System.Security.Cryptography", c"CryptoStream");
    if CryptoStream.is_null() {
        error!("Failed to get System.Security.Cryptography.CryptoStream class!");
        return InitResult::Error;
    };
    let Write_addr =  il2cpp.get_method_addr(CryptoStream, c"Write", 3);
    if FromBase64String_addr == 0 {
        error!("Failed to get Write address!");
        return InitResult::Error;
    }
    if let Some(trampoline) = interceptor.hook(Write_addr, Write as _) {
        unsafe { WRITE_ORIG = trampoline; }
    }


    InitResult::Ok
}


type FromBase64StringFn = extern "C" fn(s: *mut Il2CppString) -> *mut Il2CppArray;
unsafe extern "C" fn FromBase64String(s: *mut Il2CppString) -> *mut Il2CppArray {
    let orig_fn: FromBase64StringFn = std::mem::transmute(FROM_BASE_64_STRING_ORIG);

    let bytes = orig_fn(s);

    let arr: Array<u8> = Array::from(bytes);
    let mut datavec: Vec<u8> = Vec::new();
    unsafe {
        let slice = arr.as_slice();

        if slice.len() > *PARTIAL_MESSAGE_SIZE
        {

            let num_messages = (slice.len() / *PARTIAL_MESSAGE_SIZE) + 1;
            datavec.push(4 ); //Multipart header
            datavec.push( num_messages as u8 );

            //Send via UDP to UL
            let s = socket.lock().unwrap();
            match s.send_to(datavec.as_slice(), UL_ADDRESS.clone()) {
                Ok(_) => {}
                Err(e) => {
                    error!("Failed to send response header : {}", e);
                }
            }

            for chunk in split(slice, num_messages) {
                datavec.clear();
                datavec.push(5 ); //Multipart chunk
                datavec.push( (chunk.len() / 256) as u8 );
                datavec.push( (chunk.len() % 256) as u8 );
                datavec.extend_from_slice(chunk);
                info!( "Sending chunk" );
                //This is a hacky way to work around the buffer not having enough space
                thread::sleep(Duration::from_millis( *DELAY_BETWEEN_CHUNKS_MS ));
                match s.send_to(datavec.as_slice(), UL_ADDRESS.clone()) {
                    Ok(_) => {}
                    Err(e) => {
                        error!("Failed to send response chunk : {}", e);
                    }
                }

            }
            return bytes;
        }
        else {
            datavec.push(0);
            datavec.push( (slice.len() / 256) as u8 );
            datavec.push( (slice.len() % 256) as u8 );
            datavec.extend_from_slice(slice);
            //Send via UDP to UL
            info!( "Sending response" );
            let s = socket.lock().unwrap();
            match s.send_to(datavec.as_slice(), UL_ADDRESS.clone()) {
                Ok(_) => {}
                Err(e) => {
                    error!("Failed to send response : {}", e);
                }
            }
        }
    }

    return bytes
}

type CreateDecryptorFn = extern "C" fn(this: *mut Il2CppObject, key: *mut Il2CppArray, iv: *mut Il2CppArray) -> *mut Il2CppObject;
unsafe extern "C" fn CreateDecryptor(this: *mut Il2CppObject, key: *mut Il2CppArray, iv: *mut Il2CppArray) -> *mut Il2CppObject {
    let orig_fn: CreateDecryptorFn = std::mem::transmute(CREATE_DECRYPTOR_ORIG);


    let mut keyvec: Vec<u8> = Vec::new();
    let mut ivvec: Vec<u8> = Vec::new();
    let key_arr: Array<u8> = Array::from(key);
    let iv_arr: Array<u8> = Array::from(iv);
    unsafe
        {
            let key_slice = key_arr.as_slice();
            if key_slice.len() != 32
            {
                error!("key len is not 32");
                return orig_fn(this, key, iv)
            }
            let iv_slice = iv_arr.as_slice();
            if iv_slice.len() != 16
            {
                error!("iv len is not 16");
                return orig_fn(this, key, iv)
            }
            keyvec.push(1);
            keyvec.push( 0 );
            keyvec.push( 32 );
            keyvec.extend_from_slice(key_slice);
            //Send via UDP to UL
            info!( "Sending key" );
            match socket.lock().unwrap().send_to(keyvec.as_slice(), UL_ADDRESS.clone()) {
                Ok(_) => {}
                Err(e) => {
                    error!("Failed to send key : {}", e);
                }
            }

            //thread::sleep(Duration::from_millis(200));
            ivvec.push(2);
            ivvec.push( 0 );
            ivvec.push( 16 );
            ivvec.extend_from_slice(iv_slice);
            info!( "Sending iv" );
            match socket.lock().unwrap().send_to(ivvec.as_slice(), UL_ADDRESS.clone()) {
                Ok(_) => {}
                Err(e) => {
                    error!("Failed to send iv : {}", e);
                }
            }
        }

    orig_fn(this, key, iv)
}

type WriteFn = extern "C" fn(this: *mut Il2CppObject, buffer: *mut Il2CppArray, offset: i32, count: i32);
unsafe extern "C" fn Write(this: *mut Il2CppObject, buffer: *mut Il2CppArray, offset: i32, count: i32) {
    let orig_fn: WriteFn = std::mem::transmute(WRITE_ORIG);

    let arr: Array<u8> = Array::from(buffer);
    let mut datavec: Vec<u8> = Vec::new();
    unsafe {
        let slice = arr.as_slice();
        if slice.len() > 65535
        {
            error!( "req len is too long" );
            return orig_fn(this, buffer, offset, count);
        }
        datavec.push(3);
        datavec.push( (slice.len() / 256) as u8 );
        datavec.push( (slice.len() % 256) as u8 );
        datavec.extend_from_slice(slice);
        //Send via UDP to UL
        info!( "Sending request" );
        match socket.lock().unwrap().send_to(datavec.as_slice(), UL_ADDRESS.clone() ) {
            Ok(_) => {}
            Err(e) => {
                error!("Failed to send request : {}", e);
            }
        }

    }

    orig_fn(this, buffer, offset, count);
}




/*
Helpers
 */


fn create_default_config_file( config_file_path: &String )
{
    info!( "Creating default config file: {config_file_path}" );
    let path = std::path::Path::new(config_file_path);
    let parent = path.parent().unwrap();
    let res = std::fs::create_dir_all(parent);
    if res.is_err()
    {
        error!( "Failed to create config directory: {} : {}", parent.to_str().unwrap(), res.unwrap_err() );
        return;
    }
    let file = std::fs::File::create(config_file_path);
    if file.is_err()
    {
        error!( "Failed to create default config file: {config_file_path} : {}", file.unwrap_err() );
        return;
    }
    let res = file.unwrap().write_all( DEFAULT_CONFIG_FILE_CONTENTS );
    if res.is_err()
    {
        error!( "Failed to write to config file: {config_file_path} : {}", res.unwrap_err() );
        return;
    }
}


struct Split<'a, T> {
    slice: &'a [T],
    len: usize,
    rem: usize,
}

impl<'a, T> Iterator for Split<'a, T> {
    type Item = &'a [T];

    fn next(&mut self) -> Option<Self::Item> {
        if self.slice.is_empty() {
            return None;
        }
        let mut len = self.len;
        if self.rem > 0 {
            len += 1;
            self.rem -= 1;
        }
        let (chunk, rest) = self.slice.split_at(len);
        self.slice = rest;
        Some(chunk)
    }
}


pub fn split<T>(slice: &[T], n: usize) -> impl Iterator<Item = &[T]> {
    let len = slice.len() / n;
    let rem = slice.len() % n;
    Split { slice, len, rem }
}

