import contextlib
import importlib
import importlib.metadata
import json
import math
import os
from dataclasses import asdict, dataclass
from pathlib import Path
import platform
import sys
import tempfile


_PROTOCOL_STDOUT = sys.stdout
_DEFAULTS = {
    "model": "medium",
    "model_path": None,
    "device": "auto",
    "dtype": "auto",
    "instruments": [],
    "use_sampling": False,
    "temperature": 1,
    "beam_size": 1,
    "cfg_coef": 1,
    "strict_eos": True,
}
_FIELDS = set(_DEFAULTS) | {"action", "audio_path", "output_dir"}
_PRESET_ARCHITECTURES = {"small": (768, 14), "medium": (1024, 24), "large": (1536, 48)}


def emit(event):
    print(json.dumps(event, ensure_ascii=True, allow_nan=False), file=_PROTOCOL_STDOUT, flush=True)


def progress(stage, message, note_count=0, completed=None, total=None):
    emit({"type": "progress", "stage": stage, "message": message,
          "completed": completed, "total": total, "note_count": note_count})


def _local_path(value, field, directory=False):
    if not isinstance(value, str) or not value.strip() or "\0" in value or "://" in value:
        raise ValueError(f"{field} 必须是现有本地路径。")
    path = Path(value).resolve(strict=True)
    if not (path.is_dir() if directory else path.is_file()):
        raise ValueError(f"{field} 必须是现有{'目录' if directory else '文件'}。")
    return path


def _check_outputs(output_dir):
    for name in ("raw.mid", "events.jsonl"):
        if os.path.lexists(output_dir / name):
            raise FileExistsError(f"输出已存在，禁止覆盖：{output_dir / name}")


def validate_request(data):
    if not isinstance(data, dict):
        raise ValueError("请求必须是 JSON 对象。")
    if set(data) - _FIELDS:
        raise ValueError("请求包含不支持的字段；不能指定任意输出文件、chunk 或 token 参数。")
    request = {**_DEFAULTS, **data}
    for field, choices in (
        ("action", ("probe", "prepare_model", "transcribe")),
        ("model", ("small", "medium", "large")),
        ("device", ("auto", "cpu", "cuda")),
        ("dtype", ("auto", "float32", "float16")),
    ):
        if not isinstance(request.get(field), str) or request[field] not in choices:
            raise ValueError(f"{field} 必须是 {' | '.join(choices)}。")
    for field in ("use_sampling", "strict_eos"):
        if type(request[field]) is not bool:
            raise ValueError(f"{field} 必须是布尔值。")
    for field, lower, upper in (("temperature", 0, 2), ("cfg_coef", 0, 3), ("beam_size", 1, 4)):
        value = request[field]
        valid_type = type(value) is int if field == "beam_size" else type(value) in (int, float)
        if not valid_type or not lower <= value <= upper or not math.isfinite(value):
            raise ValueError(f"{field} 必须是 {lower}..{upper} 范围内的有限{'整数' if field == 'beam_size' else '数值'}。")
    instruments = request["instruments"]
    if not isinstance(instruments, list) or any(not isinstance(name, str) for name in instruments):
        raise ValueError("instruments 必须是乐器组键的字符串列表。")
    request["instruments"] = list(instruments)
    if request["model_path"] is not None:
        path = _local_path(request["model_path"], "model_path")
        if path.suffix.lower() != ".safetensors":
            raise ValueError("model_path 只接受本地 .safetensors 权重文件，不接受代码或包。")
        request["model_path"] = path
    for field in ("audio_path", "output_dir"):
        if request.get(field) is not None or request["action"] == "transcribe":
            request[field] = _local_path(request.get(field), field, directory=field == "output_dir")
    if request["action"] == "transcribe":
        _check_outputs(request["output_dir"])
    return request


@dataclass(frozen=True)
class Dependencies:
    torch: object
    model_class: object
    note_start: type
    note_end: type
    chunk_progress: type
    instrument_names: frozenset
    version: str


def load_dependencies():
    try:
        torch = importlib.import_module("torch")
        muscriptor = importlib.import_module("muscriptor")
        events = importlib.import_module("muscriptor.events")
        tokenizer = importlib.import_module("muscriptor.tokenizer.mt3")
        version = importlib.metadata.version("muscriptor")
    except (ImportError, importlib.metadata.PackageNotFoundError, OSError) as exc:
        raise RuntimeError(
            "无法加载 MuScriptor 依赖。请使用当前 worktree/.work/transcription 的专用 Python，"
            "并让环境准备步骤检查 MuScriptor 0.4.0、PyTorch 及其依赖/DLL。"
            f"原因：{exc}"
        ) from exc
    if version != "0.4.0":
        raise RuntimeError(f"此桥仅支持 MuScriptor 0.4.0，当前版本为 {version}；请检查专用 Python 环境。")
    return Dependencies(torch, muscriptor.TranscriptionModel, events.NoteStartEvent,
                        events.NoteEndEvent, events.ProgressEvent,
                        frozenset(tokenizer.MT3_FULL_PLUS_GROUP_NAMES), version)


def _gpu_info(torch):
    if not torch.cuda.is_available():
        return False, None, None
    properties = torch.cuda.get_device_properties(torch.cuda.current_device())
    return True, str(properties.name), int(properties.total_memory)


def _fp32_lower_bound(request):
    path = request["model_path"]
    if path is None:
        dim, layers = _PRESET_ARCHITECTURES[request["model"]]
        # Attention and feed-forward matrices alone, excluding all other weights.
        return 12 * dim * dim * layers * 4
    with path.open("rb") as stream:
        size_bytes = stream.read(8)
        if len(size_bytes) != 8:
            raise ValueError("model_path 不是有效的 safetensors 文件。")
        header_size = int.from_bytes(size_bytes, "little")
        if not 2 <= header_size <= 100_000_000 or header_size > path.stat().st_size - 8:
            raise ValueError("safetensors 头长度无效。")
        header = json.loads(stream.read(header_size))
    if not isinstance(header, dict):
        raise ValueError("safetensors 头必须是 JSON 对象。")
    count = 0
    for name, tensor in header.items():
        if not name.startswith("transformer."):
            continue
        shape = tensor.get("shape") if isinstance(tensor, dict) else None
        if not isinstance(shape, list) or any(type(n) is not int or n < 0 for n in shape):
            raise ValueError("safetensors Transformer 张量形状无效。")
        count += math.prod(shape)
    return count * 4


def load_model(request, deps):
    available, _, memory = (False, None, None) if request["device"] == "cpu" else _gpu_info(deps.torch)
    if request["device"] == "cuda" and not available:
        raise RuntimeError("请求了 CUDA，但当前 PyTorch/CUDA 无可用 GPU；请检查驱动或明确选择 CPU。")
    using_cuda = available and request["device"] != "cpu"
    if using_cuda:
        minimum = _fp32_lower_bound(request)
        if minimum > memory:
            raise RuntimeError(
                f"GPU 总显存 {memory / 2**30:.2f} GiB，低于模型仅 Transformer FP32 权重的"
                f"下界 {minimum / 2**30:.2f} GiB。MuScriptor 0.4.0 先以 FP32 构建/加载再转换 dtype；"
                "选择 float16 也不能绕过此限制。请选择更小模型或明确选择 CPU，不会自动回退。"
            )
    progress("loading", "正在加载 MuScriptor 模型。")
    model = deps.model_class.load_model(
        weights_path=request["model_path"] if request["model_path"] is not None else request["model"],
        device=None if request["device"] == "auto" else request["device"],
        dtype=None if request["dtype"] == "auto" else request["dtype"],
    )
    device = str(model._device)
    # The first parameter belongs to the FP32 conditioner, not the transformer.
    dtype = str(model._model.emb.weight.dtype).removeprefix("torch.")
    if using_cuda and device.split(":", 1)[0] != "cuda":
        raise RuntimeError("模型未加载到预期的 CUDA 设备；不允许静默 CPU 回退。")
    detail = "；CUDA AMP float16 为计算精度，不是权重精度" if device.startswith("cuda") else ""
    progress("loading", f"模型已加载：{device}，Transformer 权重 {dtype}{detail}。")
    return model, device, dtype


def _new_partial(output_dir, name):
    fd, name = tempfile.mkstemp(prefix=f".{name}.", suffix=".partial", dir=output_dir)
    return fd, Path(name)


def _sync(stream):
    stream.flush()
    os.fsync(stream.fileno())


def _publish_outputs(pairs):
    published = []
    try:
        for partial, final in pairs:
            identity = partial.stat()
            # Windows rename / POSIX link are atomic and refuse existing targets.
            if os.name == "nt":
                os.rename(partial, final)
            else:
                os.link(partial, final)
            published.append((final, identity))
    except BaseException:
        for final, identity in reversed(published):
            try:
                current = final.lstat()
                if (current.st_dev, current.st_ino) == (identity.st_dev, identity.st_ino):
                    final.unlink()
            except FileNotFoundError:
                pass
        raise


def transcribe(request, deps, model, device, dtype):
    output_dir = request["output_dir"]
    _check_outputs(output_dir)
    events = []
    instruments = set()
    note_count = 0
    partials = []
    try:
        progress("transcribing", "正在转录原始音符事件。")
        fd, events_partial = _new_partial(output_dir, "events.jsonl")
        partials.append(events_partial)
        with os.fdopen(fd, "w", encoding="utf-8", newline="\n") as stream:
            for event in model.transcribe(
                request["audio_path"],
                use_sampling=request["use_sampling"],
                temperature=request["temperature"],
                cfg_coef=request["cfg_coef"],
                instruments=request["instruments"],
                batch_size=1,
                no_eos_is_ok=not request["strict_eos"],
                beam_size=request["beam_size"],
                prelude_forcing=True,
            ):
                if isinstance(event, deps.note_start):
                    events.append(event)
                    note_count += 1
                    if event.instrument != "drums":
                        instruments.add(event.instrument)
                elif isinstance(event, deps.note_end):
                    events.append(event)
                elif isinstance(event, deps.chunk_progress):
                    progress("transcribing", "正在转录音频分块。", note_count, event.completed, event.total)
                else:
                    raise RuntimeError("MuScriptor 返回了不支持的事件类型。")
                stream.write(json.dumps({"type": type(event).__name__, **asdict(event)},
                                        ensure_ascii=False, allow_nan=False) + "\n")
            _sync(stream)
        progress("writing", "正在写出独立 MIDI 与原始事件，不进行 NBS 映射。", note_count)
        midi = model.events_to_midi_bytes(iter(events))
        if not isinstance(midi, bytes) or len(midi) < 14 or not midi.startswith(b"MThd"):
            raise RuntimeError("MuScriptor 未返回有效的非空 MThd MIDI 数据。")
        fd, midi_partial = _new_partial(output_dir, "raw.mid")
        partials.append(midi_partial)
        with os.fdopen(fd, "wb") as stream:
            stream.write(midi)
            _sync(stream)
        warnings = [
            "统一力度，未预测声像。",
            "原始表示未经过NBS映射，但官方会清理最短/重叠音。",
            "未进行量化、beat-this 节拍校时、补空隙或去除开头静音。",
            "官方 0.4.0 对最终仍未闭合的音符使用起音后 10ms fallback；公开事件不能可靠区分这些音符，本桥未擅自延长。",
        ]
        if device.startswith("cuda"):
            warnings.append(f"实际 Transformer 权重为 {dtype}；CUDA AMP 使用 float16 计算，条件处理仍为 float32。")
        if len(instruments) > 15:
            warnings.append(f"原始事件包含 {len(instruments)} 种非鼓乐器，超过 15 个非鼓 program；官方 MIDI 存在 channel 15 复用风险，音色可能冲突。")
        if note_count == 0:
            warnings.append("未识别到任何音符，MIDI 只有空轨。指定乐器组（如 acoustic_piano）后重试往往能出结果；不约束乐器时模型可能返回空。")
        midi_path = output_dir / "raw.mid"
        events_path = output_dir / "events.jsonl"
        _publish_outputs(((events_partial, events_path), (midi_partial, midi_path)))
        return {"type": "result", "midi_path": str(midi_path), "events_path": str(events_path),
                "raw_note_count": note_count, "model": request["model"], "device": device,
                "dtype": dtype, "warnings": warnings}
    finally:
        for partial in partials:
            try:
                partial.unlink(missing_ok=True)
            except OSError:
                pass


def execute(request):
    progress("loading", "正在检查 MuScriptor 0.4.0 与 PyTorch 依赖。")
    deps = load_dependencies()
    unknown = sorted(set(request["instruments"]) - deps.instrument_names)
    if unknown:
        raise ValueError(f"未知乐器组键：{', '.join(unknown)}。合法键：{', '.join(sorted(deps.instrument_names))}")
    if request["action"] == "probe":
        available, name, memory = _gpu_info(deps.torch)
        emit({"type": "ready", "python_version": platform.python_version(),
              "torch_version": str(deps.torch.__version__), "muscriptor_version": deps.version,
              "cuda_available": available, "gpu_name": name, "gpu_memory_bytes": memory})
        return
    model, device, dtype = load_model(request, deps)
    if request["action"] == "prepare_model":
        emit({"type": "model_ready", "model": request["model"], "device": device, "dtype": dtype})
    else:
        emit(transcribe(request, deps, model, device, dtype))


def _reject_constant(value):
    raise ValueError("请求 JSON 不允许 NaN 或 Infinity。")


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    with contextlib.redirect_stdout(sys.stderr):
        try:
            if len(argv) != 1:
                raise ValueError("用法：python muscriptor_worker.py request.json")
            with Path(argv[0]).open("r", encoding="utf-8-sig") as stream:
                request = validate_request(json.load(stream, parse_constant=_reject_constant))
            execute(request)
            return 0
        except Exception as exc:
            message = str(exc) or type(exc).__name__
            for key in ("HF_TOKEN", "HUGGING_FACE_HUB_TOKEN"):
                token = os.environ.get(key)
                if token:
                    message = message.replace(token, "[REDACTED]")
            emit({"type": "error", "message": message})
            return 1


if __name__ == "__main__":
    sys.exit(main())
