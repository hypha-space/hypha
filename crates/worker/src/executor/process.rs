use hypha_messages::Executor;

// arg("accelerate")
//         .arg("launch")
//         .arg("--config-file")
//         .arg(config_file)

pub (crate) fn get_process_args(executor: &Executor) -> Vec<String> {
    match executor {
        Executor::DiLoCoTransformer { .. } => {
            vec![
                "run".into(),
                "--directory".into(),
                "drivers/hypha-accelerate-driver/src".into(),
                "accelerate".into(),
                "launch".into(),
                "--config-file".into(),
                "test.yaml".into(),
                "training.py".into(),
            ]
        },
        Executor::ParameterServer {.. } => {
            vec![
                "run".into(),
                "--directory".into(),
                "drivers/hypha-accelerate-driver/src".into(),
                "dummy.py".into(),
            ]
        },
        _ => {
            vec![".".into()]
        }
        
    }
}

pub (crate) fn get_process_call(executor: &Executor) -> String {
    match executor {
        Executor::DiLoCoTransformer { .. } => {
            "uv".into()
        },
        Executor::ParameterServer {.. } => {
            "uv".into()
        },
        _ => {
            ".".into()
        }
        
    }
}