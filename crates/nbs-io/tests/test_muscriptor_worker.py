import contextlib
from dataclasses import asdict, dataclass
import importlib.util
import io
import json
import os
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock


WORKER_PATH = Path(__file__).resolve().parents[1] / "src" / "muscriptor_worker.py"
SPEC = importlib.util.spec_from_file_location("muscriptor_worker_under_test", WORKER_PATH)
worker = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = worker
SPEC.loader.exec_module(worker)


@dataclass
class NoteStartEvent:
    pitch: int
    start_time: float
    index: int
    instrument: str


@dataclass
class NoteEndEvent:
    end_time: float
    start_event: NoteStartEvent


@dataclass
class ProgressEvent:
    completed: int
    total: int


INSTRUMENTS = (
    "acoustic_piano", "electric_piano", "chromatic_percussion", "organ",
    "acoustic_guitar", "clean_electric_guitar", "distorted_electric_guitar",
    "acoustic_bass", "electric_bass", "violin", "viola", "cello", "contrabass",
    "orchestral_harp", "timpani", "string_ensemble", "synth_strings", "voice",
    "orchestra_hit", "trumpet", "trombone", "tuba", "french_horn", "brass_section",
    "soprano_and_alto_sax", "tenor_sax", "baritone_sax", "oboe", "english_horn",
    "bassoon", "clarinet", "flutes", "synth_lead", "synth_pad", "drums",
)
# Fixed MIDI fixture, not a model inference result.
TRACK = b"\x00\xc0\x00\x00\x90\x3c\x64\x83\x60\x80\x3c\x00\x00\xff\x2f\x00"
MIDI = b"MThd\x00\x00\x00\x06\x00\x00\x00\x01\x01\xe0MTrk" + len(TRACK).to_bytes(4, "big") + TRACK


class WorkerTests(unittest.TestCase):
    def setUp(self):
        self.assertNotIn("torch", sys.modules, "These tests must never import real torch")
        temp = tempfile.TemporaryDirectory(prefix=".worker-test-", dir=Path(__file__).resolve().parent)
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.output = self.root / "输出"
        self.output.mkdir()
        self.audio = self.root / "原始音频.wav"
        self.audio.write_bytes(b"audio fixture")
        self.request_path = self.root / "request.json"
        start = NoteStartEvent(73, 1.234567, 0, "violin")
        self.events = [ProgressEvent(0, 2), start, ProgressEvent(1, 2),
                       NoteEndEvent(2.345678, start), ProgressEvent(2, 2)]
        self.converted_events = None
        self.model = SimpleNamespace(
            _device="cpu",
            _model=SimpleNamespace(emb=SimpleNamespace(weight=SimpleNamespace(dtype="torch.float32"))),
            transcribe=mock.Mock(side_effect=lambda *args, **kwargs: iter(self.events)),
            events_to_midi_bytes=mock.Mock(side_effect=self.convert),
            transcribe_and_postprocess=mock.Mock(side_effect=AssertionError("No postprocessing")),
            detect_beat_grid_for=mock.Mock(side_effect=AssertionError("No beat detection")),
        )
        self.model_class = SimpleNamespace(load_model=mock.Mock(return_value=self.model))
        self.torch = SimpleNamespace(__version__="test-torch", cuda=SimpleNamespace(
            is_available=mock.Mock(return_value=False),
            current_device=mock.Mock(return_value=0),
            get_device_properties=mock.Mock(return_value=SimpleNamespace(name="Stub GPU", total_memory=4 * 2**30)),
        ))
        modules = {
            "torch": self.torch,
            "muscriptor": SimpleNamespace(TranscriptionModel=self.model_class),
            "muscriptor.events": SimpleNamespace(NoteStartEvent=NoteStartEvent,
                                                NoteEndEvent=NoteEndEvent, ProgressEvent=ProgressEvent),
            "muscriptor.tokenizer.mt3": SimpleNamespace(MT3_FULL_PLUS_GROUP_NAMES=dict.fromkeys(INSTRUMENTS)),
        }
        self.imports = mock.patch.object(worker.importlib, "import_module", side_effect=modules.__getitem__).start()
        self.addCleanup(mock.patch.stopall)
        self.version = mock.patch.object(worker.importlib.metadata, "version", return_value="0.4.0").start()
        self.protocol = io.StringIO()
        self.logs = io.StringIO()
        mock.patch.object(worker, "_PROTOCOL_STDOUT", self.protocol).start()

    def tearDown(self):
        self.assertNotIn("torch", sys.modules)

    def convert(self, events):
        self.converted_events = list(events)
        return MIDI

    def request(self, **changes):
        return {"action": "transcribe", "audio_path": str(self.audio),
                "output_dir": str(self.output), **changes}

    def run_request(self, request):
        self.protocol.seek(0)
        self.protocol.truncate()
        self.request_path.write_text(json.dumps(request, ensure_ascii=False), encoding="utf-8")
        with contextlib.redirect_stderr(self.logs):
            code = worker.main([str(self.request_path)])
        messages = [json.loads(line) for line in self.protocol.getvalue().splitlines()]
        return code, messages

    def assert_failed(self, code, messages):
        self.assertNotEqual(code, 0)
        self.assertEqual(messages[-1]["type"], "error")
        self.assertFalse(any(m["type"] in ("result", "model_ready", "ready") for m in messages))

    def assert_no_outputs(self):
        self.assertEqual(list(self.output.iterdir()), [])

    def local_weights(self):
        header = json.dumps({"transformer.example": {"dtype": "F32", "shape": [2, 3],
                                                      "data_offsets": [0, 24]}}).encode()
        path = self.root / "local.safetensors"
        path.write_bytes(len(header).to_bytes(8, "little") + header + b"\0" * 24)
        return path

    def test_defaults(self):
        request = worker.validate_request(self.request())
        self.assertEqual(request["model"], "medium")
        self.assertEqual(request["device"], "auto")
        self.assertEqual(request["dtype"], "auto")
        self.assertEqual(request["instruments"], [])
        self.assertTrue(request["strict_eos"])
        self.assertFalse(request["use_sampling"])
        self.assertEqual(request["audio_path"], self.audio.resolve())
        self.assertEqual(request["output_dir"], self.output.resolve())

    def test_invalid_requests_never_import_dependencies(self):
        cases = [None, [], True, {}, {"action": "download"}, {"action": ["probe"]}]
        for field, values in {
            "model": [None, True, "extra-large"], "device": [1, "cuda:0", "mps"],
            "dtype": [False, "bfloat16"], "instruments": [None, "violin", [1], [False]],
            "use_sampling": [1, "false", None], "strict_eos": [0, "true"],
            "temperature": [True, -0.1, 2.1, "1", float("nan"), float("inf"), 10**400],
            "cfg_coef": [False, -1, 3.1, float("-inf")],
            "beam_size": [True, 0, 5, 1.0, "2"],
        }.items():
            cases.extend(self.request(**{field: value}) for value in values)
        cases.extend(self.request(**{field: 1}) for field in (
            "chunk_size", "max_tokens", "batch_size", "prelude_forcing", "midi_path", "quantize"))
        for data in cases:
            with self.subTest(data=data):
                code, messages = self.run_request(data)
                self.assert_failed(code, messages)
        self.imports.assert_not_called()
        self.model_class.load_model.assert_not_called()
        self.assert_no_outputs()

    def test_parameter_boundaries(self):
        for temperature in (0, 2, 0.5):
            for cfg in (0, 3, 0.5):
                for beam in (1, 4):
                    request = worker.validate_request(self.request(temperature=temperature, cfg_coef=cfg, beam_size=beam))
                    self.assertEqual(request["temperature"], temperature)
                    self.assertEqual(request["cfg_coef"], cfg)
                    self.assertEqual(request["beam_size"], beam)

    def test_required_paths_and_safe_model_path(self):
        executable = self.root / "model.py"
        executable.write_text("raise AssertionError('Do not execute')", encoding="utf-8")
        cases = [
            {"action": "transcribe"},
            self.request(audio_path=str(self.root / "missing.wav")),
            self.request(audio_path=str(self.root)), self.request(audio_path=None),
            self.request(output_dir=str(self.root / "missing")), self.request(output_dir=str(self.audio)),
            self.request(model_path="hf://MuScriptor/muscriptor-medium/model.safetensors"),
            self.request(model_path="https://example.invalid/model.safetensors"),
            self.request(model_path=str(executable)), self.request(model_path=str(self.root)),
            self.request(model_path=str(self.root / "missing.safetensors")), self.request(model_path=False),
        ]
        for data in cases:
            with self.subTest(data=data):
                self.assert_failed(*self.run_request(data))
        self.imports.assert_not_called()
        self.assert_no_outputs()

    def test_unknown_instrument_fails_before_model_load(self):
        code, messages = self.run_request(self.request(instruments=["piano"]))
        self.assert_failed(code, messages)
        self.assertIn("piano", messages[-1]["message"])
        self.assertIn("acoustic_piano", messages[-1]["message"])
        self.model_class.load_model.assert_not_called()
        self.assert_no_outputs()

    def test_all_official_instrument_keys_accepted(self):
        self.assertEqual(len(INSTRUMENTS), 35)
        code, messages = self.run_request(self.request(instruments=list(INSTRUMENTS)))
        self.assertEqual(code, 0)
        self.assertEqual(messages[-1]["type"], "result")
        self.assertEqual(self.model.transcribe.call_args.kwargs["instruments"], list(INSTRUMENTS))

    def test_probe_never_loads_or_downloads_a_model(self):
        self.torch.cuda.is_available.return_value = True
        code, messages = self.run_request({"action": "probe", "model": "large", "device": "cuda"})
        self.assertEqual(code, 0)
        self.assertEqual(messages[-1], {"type": "ready", "python_version": worker.platform.python_version(),
                                       "torch_version": "test-torch", "muscriptor_version": "0.4.0",
                                       "cuda_available": True, "gpu_name": "Stub GPU",
                                       "gpu_memory_bytes": 4 * 2**30})
        self.model_class.load_model.assert_not_called()
        self.assert_no_outputs()

    def test_cpu_probe_has_null_gpu_fields(self):
        code, messages = self.run_request({"action": "probe"})
        self.assertEqual(code, 0)
        self.assertFalse(messages[-1]["cuda_available"])
        self.assertIsNone(messages[-1]["gpu_name"])
        self.assertIsNone(messages[-1]["gpu_memory_bytes"])
        self.torch.cuda.get_device_properties.assert_not_called()
        self.model_class.load_model.assert_not_called()

    def test_probe_accepts_null_transcription_only_paths(self):
        code, messages = self.run_request({"action": "probe", "audio_path": None, "output_dir": None})
        self.assertEqual(code, 0)
        self.assertEqual(messages[-1]["type"], "ready")
        self.model_class.load_model.assert_not_called()

    def test_protocol_is_ascii_safe_json_and_always_flushed(self):
        stream = mock.Mock()
        with mock.patch.object(worker, "_PROTOCOL_STDOUT", stream):
            worker.emit({"type": "error", "message": "测试音频"})
        encoded = "".join(call.args[0] for call in stream.write.call_args_list)
        self.assertTrue(encoded.isascii())
        self.assertEqual(json.loads(encoded)["message"], "测试音频")
        stream.flush.assert_called_once_with()

    def test_missing_dependency_has_actionable_error(self):
        self.imports.side_effect = ModuleNotFoundError("No module named 'torch'", name="torch")
        code, messages = self.run_request({"action": "probe"})
        self.assert_failed(code, messages)
        self.assertIn(".work/transcription", messages[-1]["message"])
        self.assertIn("torch", messages[-1]["message"])
        self.model_class.load_model.assert_not_called()

    def test_wrong_package_version_rejected(self):
        self.version.return_value = "0.3.0"
        code, messages = self.run_request({"action": "probe"})
        self.assert_failed(code, messages)
        self.assertIn("0.4.0", messages[-1]["message"])
        self.model_class.load_model.assert_not_called()

    def test_prepare_uses_local_safetensors_and_actual_weight_dtype(self):
        path = self.local_weights()
        self.model._model.emb.weight.dtype = "torch.float16"
        code, messages = self.run_request({"action": "prepare_model", "model_path": str(path),
                                           "device": "cpu", "dtype": "float16"})
        self.assertEqual(code, 0)
        self.model_class.load_model.assert_called_once_with(weights_path=path.resolve(), device="cpu", dtype="float16")
        self.assertEqual(messages[-1], {"type": "model_ready", "model": "medium", "device": "cpu", "dtype": "float16"})
        self.model.transcribe.assert_not_called()
        self.assert_no_outputs()

    def test_auto_passes_none_and_preserves_cuda_fp32_weights(self):
        self.torch.cuda.is_available.return_value = True
        self.model._device = "cuda:0"
        code, messages = self.run_request({"action": "prepare_model"})
        self.assertEqual(code, 0)
        self.model_class.load_model.assert_called_once_with(weights_path="medium", device=None, dtype=None)
        self.assertEqual(messages[-1]["dtype"], "float32")
        self.assertEqual(messages[-1]["device"], "cuda:0")
        self.assertTrue(any("AMP float16" in m.get("message", "") for m in messages))

    def test_unavailable_cuda_does_not_fall_back(self):
        code, messages = self.run_request({"action": "prepare_model", "device": "cuda"})
        self.assert_failed(code, messages)
        self.assertIn("CUDA", messages[-1]["message"])
        self.model_class.load_model.assert_not_called()

    def test_large_rejected_on_4gib_even_for_float16(self):
        self.torch.cuda.is_available.return_value = True
        for dtype in ("auto", "float32", "float16"):
            for device in ("auto", "cuda"):
                with self.subTest(dtype=dtype, device=device):
                    code, messages = self.run_request({"action": "prepare_model", "model": "large", "dtype": dtype, "device": device})
                    self.assert_failed(code, messages)
                    self.assertIn("FP32", messages[-1]["message"])
                    self.assertIn("4.00 GiB", messages[-1]["message"])
        self.model_class.load_model.assert_not_called()

    def test_explicit_cpu_bypasses_gpu_capacity_guard(self):
        self.torch.cuda.is_available.return_value = True
        code, messages = self.run_request({"action": "prepare_model", "model": "large", "device": "cpu"})
        self.assertEqual(code, 0)
        self.assertEqual(messages[-1]["type"], "model_ready")
        self.model_class.load_model.assert_called_once_with(weights_path="large", device="cpu", dtype=None)

    def test_local_safetensors_memory_bound_uses_header_not_label(self):
        path = self.local_weights()
        request = worker.validate_request({"action": "prepare_model", "model": "large", "model_path": str(path)})
        self.assertEqual(worker._fp32_lower_bound(request), 24)
        self.torch.cuda.is_available.return_value = True
        self.model._device = "cuda:0"
        code, messages = self.run_request({"action": "prepare_model", "model": "large", "model_path": str(path)})
        self.assertEqual(code, 0)
        self.assertEqual(messages[-1]["type"], "model_ready")
        self.assertEqual(self.model_class.load_model.call_args.kwargs["weights_path"], path.resolve())

    def test_malformed_local_safetensors_fails_before_cuda_load(self):
        self.torch.cuda.is_available.return_value = True
        path = self.root / "bad.safetensors"
        for content in (b"", b"invalid!", (100_000_001).to_bytes(8, "little")):
            with self.subTest(content=content):
                path.write_bytes(content)
                self.assert_failed(*self.run_request({"action": "prepare_model", "model_path": str(path)}))
        self.model_class.load_model.assert_not_called()

    def test_unexpected_cpu_fallback_rejected(self):
        self.torch.cuda.is_available.return_value = True
        self.assert_failed(*self.run_request({"action": "prepare_model"}))
        self.model.transcribe.assert_not_called()

    def test_raw_events_to_midi_without_quantization_or_postprocessing(self):
        with mock.patch.object(worker.os, "fsync", wraps=os.fsync) as fsync:
            code, messages = self.run_request(self.request())
        self.assertEqual(code, 0)
        self.assertEqual(fsync.call_count, 2)
        result = messages[-1]
        self.assertEqual(result["type"], "result")
        self.assertEqual(result["raw_note_count"], 1)
        self.assertEqual(Path(result["midi_path"]), self.output.resolve() / "raw.mid")
        self.assertEqual(Path(result["events_path"]), self.output.resolve() / "events.jsonl")
        self.assertEqual(Path(result["midi_path"]).read_bytes(), MIDI)
        saved = [json.loads(line) for line in Path(result["events_path"]).read_text(encoding="utf-8").splitlines()]
        self.assertEqual(saved, [{"type": type(event).__name__, **asdict(event)} for event in self.events])
        notes = [event for event in self.events if not isinstance(event, ProgressEvent)]
        self.assertEqual(self.converted_events, notes)
        for original, converted in zip(notes, self.converted_events):
            self.assertIs(original, converted)
        self.model.transcribe.assert_called_once_with(self.audio.resolve(), use_sampling=False,
            temperature=1, cfg_coef=1, instruments=[], batch_size=1, no_eos_is_ok=False,
            beam_size=1, prelude_forcing=True)
        self.assertEqual(self.model.events_to_midi_bytes.call_args.kwargs, {})
        self.model.transcribe_and_postprocess.assert_not_called()
        self.model.detect_beat_grid_for.assert_not_called()
        anchors = [m for m in messages if m["type"] == "progress" and m["completed"] is not None]
        self.assertEqual([(m["completed"], m["total"], m["note_count"]) for m in anchors], [(0, 2, 0), (1, 2, 1), (2, 2, 1)])
        self.assertEqual({m["type"] for m in messages}, {"progress", "result"})
        self.assertEqual(set(p.name for p in self.output.iterdir()), {"raw.mid", "events.jsonl"})
        warnings = " ".join(result["warnings"])
        for phrase in ("统一力度", "未预测声像", "NBS", "最短/重叠", "10ms", "未擅自延长"):
            self.assertIn(phrase, warnings)
        self.assertEqual(self.audio.read_bytes(), b"audio fixture")

    def test_sampling_and_relaxed_eos_are_forwarded(self):
        code, _ = self.run_request(self.request(use_sampling=True, temperature=0.5,
                                               cfg_coef=2, beam_size=4, strict_eos=False))
        self.assertEqual(code, 0)
        kwargs = self.model.transcribe.call_args.kwargs
        self.assertTrue(kwargs["use_sampling"])
        self.assertTrue(kwargs["no_eos_is_ok"])
        self.assertEqual((kwargs["temperature"], kwargs["cfg_coef"], kwargs["beam_size"]), (0.5, 2, 4))
        self.assertEqual(kwargs["batch_size"], 1)
        self.assertTrue(kwargs["prelude_forcing"])

    def test_channel_reuse_warning_excludes_drums(self):
        for count in (15, 16):
            with self.subTest(count=count):
                self.events = []
                for index, name in enumerate(list(INSTRUMENTS[:count]) + ["drums"]):
                    start = NoteStartEvent(60, 0.1, index, name)
                    self.events.extend((start, NoteEndEvent(0.11, start)))
                code, messages = self.run_request(self.request())
                self.assertEqual(code, 0)
                warnings = " ".join(messages[-1]["warnings"])
                self.assertEqual("channel 15" in warnings, count > 15)
                (self.output / "raw.mid").unlink()
                (self.output / "events.jsonl").unlink()

    def test_zero_note_result_still_publishes_and_warns(self):
        self.events = [ProgressEvent(1, 1)]
        code, messages = self.run_request(self.request())
        self.assertEqual(code, 0)
        result = messages[-1]
        self.assertEqual(result["raw_note_count"], 0)
        self.assertEqual(Path(result["midi_path"]).read_bytes(), MIDI)
        warnings = " ".join(result["warnings"])
        self.assertIn("未识别到任何音符", warnings)
        self.assertIn("acoustic_piano", warnings)
        (self.output / "raw.mid").unlink()
        (self.output / "events.jsonl").unlink()

    def test_model_load_failure_has_no_completion(self):
        self.model_class.load_model.side_effect = RuntimeError("out of memory")
        self.assert_failed(*self.run_request(self.request()))
        self.model.transcribe.assert_not_called()
        self.assert_no_outputs()

    def test_inference_failure_cleans_partial_and_never_writes_midi(self):
        def failing_events(*args, **kwargs):
            yield self.events[1]
            raise RuntimeError("missing EOS")
        self.model.transcribe.side_effect = failing_events
        self.assert_failed(*self.run_request(self.request()))
        self.model.events_to_midi_bytes.assert_not_called()
        self.assert_no_outputs()

    def test_midi_conversion_failure_has_no_final_files(self):
        self.model.events_to_midi_bytes.side_effect = RuntimeError("MIDI writer failure")
        self.assert_failed(*self.run_request(self.request()))
        self.assert_no_outputs()

    def test_empty_or_invalid_midi_has_no_final_files(self):
        self.model.events_to_midi_bytes.side_effect = None
        for midi in (b"", b"MThd", b"invalid header" * 2, "MThd" * 20):
            with self.subTest(midi=midi):
                self.model.events_to_midi_bytes.return_value = midi
                self.assert_failed(*self.run_request(self.request()))
                self.assert_no_outputs()

    def test_invalid_event_has_no_final_files(self):
        for event in (object(), NoteStartEvent(60, float("nan"), 0, "violin")):
            with self.subTest(event=event):
                self.events = [event]
                self.assert_failed(*self.run_request(self.request()))
                self.assert_no_outputs()
        self.model.events_to_midi_bytes.assert_not_called()

    def test_fsync_failure_has_no_final_files(self):
        with mock.patch.object(worker.os, "fsync", side_effect=OSError("disk full")):
            self.assert_failed(*self.run_request(self.request()))
        self.assert_no_outputs()

    def test_existing_outputs_protected_before_model_load(self):
        for name in ("raw.mid", "events.jsonl"):
            with self.subTest(name=name):
                path = self.output / name
                path.write_bytes(b"user data")
                self.assert_failed(*self.run_request(self.request()))
                self.assertEqual(path.read_bytes(), b"user data")
                path.unlink()
        self.imports.assert_not_called()
        self.model_class.load_model.assert_not_called()
        self.assert_no_outputs()

    def test_source_at_final_name_is_never_overwritten(self):
        source = self.output / "raw.mid"
        source.write_bytes(b"source data")
        self.assert_failed(*self.run_request(self.request(audio_path=str(source))))
        self.assertEqual(source.read_bytes(), b"source data")
        self.model_class.load_model.assert_not_called()

    def test_conflict_during_publish_preserves_other_files_and_rolls_back(self):
        for name in ("raw.mid", "events.jsonl"):
            with self.subTest(name=name):
                path = self.output / name
                def introduce_conflict(events):
                    path.write_bytes(b"concurrent user file")
                    return self.convert(events)
                self.model.events_to_midi_bytes.side_effect = introduce_conflict
                self.assert_failed(*self.run_request(self.request()))
                self.assertEqual(path.read_bytes(), b"concurrent user file")
                self.assertEqual(list(self.output.iterdir()), [path])
                path.unlink()

    def test_third_party_stdout_is_redirected_and_environment_unchanged(self):
        def noisy_load(**kwargs):
            print("third-party ordinary output")
            return self.model
        self.model_class.load_model.side_effect = noisy_load
        environment = {"HF_HOME": "isolated-hf", "TORCH_HOME": "isolated-torch", "TMP": "isolated-tmp"}
        with mock.patch.dict(os.environ, environment):
            before = dict(os.environ)
            code, messages = self.run_request({"action": "prepare_model"})
            self.assertEqual(dict(os.environ), before)
        self.assertEqual(code, 0)
        self.assertEqual(messages[-1]["type"], "model_ready")
        self.assertIn("third-party ordinary output", self.logs.getvalue())
        self.assertNotIn("third-party ordinary output", self.protocol.getvalue())

    def test_error_redacts_hf_token(self):
        token = "hf_secret_test_value"
        self.model_class.load_model.side_effect = RuntimeError("failed with " + token)
        with mock.patch.dict(os.environ, {"HF_TOKEN": token}):
            code, messages = self.run_request({"action": "prepare_model"})
        self.assert_failed(code, messages)
        self.assertNotIn(token, self.protocol.getvalue() + self.logs.getvalue())
        self.assertIn("[REDACTED]", messages[-1]["message"])

    def test_json_encoding_errors_and_usage_return_nonzero(self):
        for content in (b"{", b"\xff\xff", b'{"action": "probe", "temperature": NaN}'):
            self.protocol.seek(0)
            self.protocol.truncate()
            self.request_path.write_bytes(content)
            code = worker.main([str(self.request_path)])
            messages = [json.loads(line) for line in self.protocol.getvalue().splitlines()]
            self.assert_failed(code, messages)
        for args in ([], [str(self.request_path), "extra"]):
            self.protocol.seek(0)
            self.protocol.truncate()
            code = worker.main(args)
            self.assert_failed(code, [json.loads(line) for line in self.protocol.getvalue().splitlines()])
        self.imports.assert_not_called()


if __name__ == "__main__":
    unittest.main()
