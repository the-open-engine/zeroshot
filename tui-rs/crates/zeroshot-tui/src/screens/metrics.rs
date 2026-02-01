use crate::protocol::ClusterMetrics;

pub const CPU_COLUMN_WIDTH: usize = 6;
pub const MEM_COLUMN_WIDTH: usize = 8;
const PLACEHOLDER: &str = "-";

pub fn format_cpu_percent(metrics: Option<&ClusterMetrics>) -> String {
    let Some(metrics) = metrics else {
        return format_placeholder(CPU_COLUMN_WIDTH);
    };
    if !metrics.supported {
        return format_placeholder(CPU_COLUMN_WIDTH);
    }
    let Some(value) = metrics.cpu_percent else {
        return format_placeholder(CPU_COLUMN_WIDTH);
    };
    if !value.is_finite() {
        return format_placeholder(CPU_COLUMN_WIDTH);
    }
    format!("{:>width$.1}%", value, width = CPU_COLUMN_WIDTH - 1)
}

pub fn format_memory_mb(metrics: Option<&ClusterMetrics>) -> String {
    let Some(metrics) = metrics else {
        return format_placeholder(MEM_COLUMN_WIDTH);
    };
    if !metrics.supported {
        return format_placeholder(MEM_COLUMN_WIDTH);
    }
    let Some(value) = metrics.memory_mb else {
        return format_placeholder(MEM_COLUMN_WIDTH);
    };
    if !value.is_finite() {
        return format_placeholder(MEM_COLUMN_WIDTH);
    }
    let rounded = value.round() as i64;
    format!("{:>width$}MB", rounded, width = MEM_COLUMN_WIDTH - 2)
}

pub fn format_metrics_line(metrics: Option<&ClusterMetrics>) -> String {
    let cpu = format_cpu_percent(metrics);
    let mem = format_memory_mb(metrics);
    format!("CPU {cpu} | MEM {mem}")
}

fn format_placeholder(width: usize) -> String {
    format!("{:>width$}", PLACEHOLDER, width = width)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics(supported: bool, cpu: Option<f64>, mem: Option<f64>) -> ClusterMetrics {
        ClusterMetrics {
            id: "cluster-1".to_string(),
            supported,
            cpu_percent: cpu,
            memory_mb: mem,
        }
    }

    #[test]
    fn placeholder_when_unsupported() {
        let sample = metrics(false, Some(12.3), Some(456.7));
        let cpu = format_cpu_percent(Some(&sample));
        assert_eq!(cpu.trim(), PLACEHOLDER);
        assert_eq!(cpu.len(), CPU_COLUMN_WIDTH);

        let mem = format_memory_mb(Some(&sample));
        assert_eq!(mem.trim(), PLACEHOLDER);
        assert_eq!(mem.len(), MEM_COLUMN_WIDTH);
    }

    #[test]
    fn cpu_rounds_to_one_decimal() {
        let sample = metrics(true, Some(12.34), None);
        assert_eq!(format_cpu_percent(Some(&sample)), " 12.3%");
        let sample = metrics(true, Some(12.36), None);
        assert_eq!(format_cpu_percent(Some(&sample)), " 12.4%");
    }

    #[test]
    fn memory_rounds_to_nearest_mb() {
        let sample = metrics(true, None, Some(256.4));
        assert_eq!(format_memory_mb(Some(&sample)), "   256MB");
        let sample = metrics(true, None, Some(256.6));
        assert_eq!(format_memory_mb(Some(&sample)), "   257MB");
    }
}
